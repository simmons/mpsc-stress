//! This program demonstrates a race condition in the Futures 0.1.19 MPSC implementation.  If a
//! send occurs concurrently with the receiver close/drop, a successful send may be indicated even
//! though the item is actually stuck in a disconnected channel (until the Sender is dropped).
//!
//! This program continually tries to recreate this race condition in a hostile scenario where a
//! busy-polled future polls the MPSC channel a number of times before disconnecting, while an
//! external thread busy-loops over sends.  The errant condition is detected by observing that the
//! previously submitted item is still allocated when the Sender indicates the channel has been
//! disconnected.  When this occurs, the program will output a line similar to the following before
//! exiting:
//!
//! RACE CONDITION DETECTED on test iteration 465 after 115 send attempts (7 successful).
//!

extern crate futures;
extern crate tokio_core;

use std::thread;
use std::sync::{Arc, Weak};
use std::time::Duration;
use futures::{Async, Future, Poll, Stream};
use futures::sync::mpsc;
use futures::sync::oneshot;
use tokio_core::reactor::Core;

/// How many times will we run the test before giving up?
const ITERATION_COUNT: usize = 1000000;

/// To provide variable timing characteristics (in the hopes of reproducing the collision that
/// leads to a race), we busy-re-poll the test MPSC receiver a variable number of times before
/// actually stopping.  We vary this countdown between 1 and the following value.
const MAX_COUNTDOWN: usize = 20;

/// Each test iteration consists of an MPSC receiver submitted to the future and queried in a
/// busy-loop the specified number of times before dropping the receiver.
#[derive(Debug)]
struct Test(mpsc::Receiver<Arc<()>>, usize);

/// Our test future is the main future run on the Tokio reactor, and persists throughout the
/// lifetime of the program.  It employs a "command" channel by which the main thread can submit an
/// MPSC channel for testing, then busy-polls this test channel a number of times before
/// disconnecting.  Items received from the channel are dropped immediately.
struct TestFuture {
    command_rx: mpsc::Receiver<Test>,
    test_rx: Option<mpsc::Receiver<Arc<()>>>,
    test_countdown: usize,
}

impl TestFuture {
    /// Create a new TestFuture
    fn new() -> (TestFuture, mpsc::Sender<Test>) {
        let (command_tx, command_rx) = mpsc::channel::<Test>(0);
        (
            TestFuture {
                command_rx,
                test_rx: None,
                test_countdown: 0, // 0 means no countdown is in progress.
            },
            command_tx,
        )
    }
}

impl Future for TestFuture {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        // Poll the test channel, if one is present.
        if let Some(ref mut test_rx) = self.test_rx {
            match test_rx.poll() {
                Ok(Async::Ready(Some(_))) => {}
                Ok(Async::Ready(None)) => panic!("test_rx unexpectedly completed."),
                Ok(Async::NotReady) => {}
                Err(_) => {}
            }
            self.test_countdown -= 1;
            // Busy-poll until the countdown is finished.
            futures::task::current().notify();
        }
        if self.test_countdown == 1 {
            // Countdown complete -- drop the Receiver.
            self.test_countdown = 0;
            self.test_rx = None;
        }

        // Accept any newly submitted MPSC channels for testing.
        if let Ok(Async::Ready(Some(Test(test_rx, countdown)))) = self.command_rx.poll() {
            self.test_rx = Some(test_rx);
            self.test_countdown = countdown + 1;

            // We're technically obligated to poll command_rx until it returns NotReady.  Instead,
            // we'll just arrange to be polled again immediately as long as command_rx is Ready.
            futures::task::current().notify();
        }

        Ok(Async::NotReady)
    }
}

/// Spawn a Tokio reactor thread running our future.
fn reactor_thread() -> mpsc::Sender<Test> {
    let (tx, rx) = oneshot::channel::<mpsc::Sender<Test>>();
    thread::spawn(move || {
        let mut core = Core::new().unwrap();
        let (future, cmd_tx) = TestFuture::new();
        tx.send(cmd_tx).unwrap();
        core.run(future).unwrap();
    });
    let cmd_tx = rx.wait().unwrap();
    cmd_tx
}

fn main() {
    let mut next_stop_countdown = 1;

    // Spawn the Tokio reactor thread with our future.
    let mut command_tx = reactor_thread();

    for i in 0..ITERATION_COUNT {
        // Create a new MPSC channel for testing.
        let (mut test_tx, test_rx) = mpsc::channel::<Arc<()>>(0);

        // Determine the countdown to be used for this test.
        let countdown = next_stop_countdown;
        next_stop_countdown = if next_stop_countdown == MAX_COUNTDOWN {
            1
        } else {
            next_stop_countdown + 1
        };

        // Submit the test MPSC receiver to the TestFuture
        command_tx.try_send(Test(test_rx, countdown)).unwrap();

        // Busy-loop sending items to the test MPSC channel until it is disconnected.
        let mut previous_weak: Option<Weak<()>> = None;
        let mut attempted_send_count: usize = 0;
        let mut successful_send_count: usize = 0;
        loop {
            // Create a test item.
            let item = Arc::new(());
            let weak = Arc::downgrade(&item);

            match test_tx.try_send(item) {
                Ok(_) => {
                    previous_weak = Some(weak);
                    successful_send_count += 1;
                }
                Err(ref e) if e.is_full() => {}
                Err(ref e) if e.is_disconnected() => {
                    // Test for evidence of the race condition.
                    if let Some(previous_weak) = previous_weak {
                        if let Some(_) = previous_weak.upgrade() {
                            // The receiver end of the channel has been dropped, and yet the
                            // previously sent item is still allocated.  This is due to the race
                            // condition in the MPSC implementation whereby a sent item can be
                            // accepted into the channel after the receiver is closed.  The item
                            // will not be dropped until the Sender is dropped.

                            // Just to demonstrate that this condition is persistent, we will sleep
                            // for one second and confirm again that the item has not been dropped.
                            thread::sleep(Duration::from_secs(1));
                            if let Some(_) = previous_weak.upgrade() {
                                println!(
                                    "RACE CONDITION DETECTED on test iteration {} \
                                     after {} send attempts ({} successful).",
                                    i, attempted_send_count, successful_send_count
                                );
                                // Ungracefully exit with an error indication.
                                std::process::exit(1);
                            }
                        }
                    }
                    break;
                }
                Err(ref e) => panic!("unexpected error: {}", e),
            }
            attempted_send_count += 1;
        }
    }
    println!("Normal completion");
}
