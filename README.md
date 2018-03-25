
Futures MPSC stress test
========================================

This program demonstrates a race condition in the Futures 0.1.19 MPSC
implementation.  If a `try_send()` occurs concurrently with the
`Receiver` close/drop, a successful send may be indicated even though
the item is actually stuck in a disconnected channel (until the `Sender`
is dropped).

This program continually tries to recreate this race condition in a
hostile scenario where a busy-polled future polls the MPSC channel a
number of times before disconnecting, while an external thread
busy-loops over sends.  The errant condition is detected by observing
that the previously submitted item is still allocated when the `Sender`
indicates the channel has been disconnected.  When this occurs, the
program will output a line similar to the following before exiting:

```
RACE CONDITION DETECTED on test iteration 465 after 115 send attempts (7 successful).
```
