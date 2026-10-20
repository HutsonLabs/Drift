# HOST-5 — One-slot newest-frame mailbox

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: HOST-5 (`TASKS.md`); ADR `IOS-0` decision 4. The idea is from Sill; no code is copied.

## Decisions

1. **`drift_core::mailbox`**, pure and generic: `mailbox()` returns a `Poster` and a `Taker`
   sharing one slot behind a mutex and a condition variable. It sits between capture and encode
   and between encode and the socket; HOST-2 wires the stages.
2. **Newest wins, nothing queues.** `post` replaces an unread item and hands it back (so a
   buffer can be reused); `try_take`, `take`, `take_timeout` and the executor-neutral async
   `recv` always get the newest item. A replaced item counts as dropped.
3. **Damage is never lost.** Dropping a frame would lose its changed regions, so
   `post_merging` folds the unread item into the new one first. `drift-capture`'s
   `CaptureStream::start_into` uses it with `Damage::merge` (a size change makes the merged
   frame a full update).
4. **Stats** (`posted`, `taken`, `dropped`, with `taken + dropped ≤ posted`) are readable from
   both ends, for the HUD and for HOST-7's backlog detection.
5. **Closing.** Dropping either end closes the mailbox: the taker still gets an item posted
   before the close, then `None`; a blocked taker wakes; the poster gets `Closed(item)` back.
   A poisoned lock is still used (every update is a single assignment).

## Consequences

- Tested with unit tests, a two-thread run of 100,000 posts (strictly increasing items, the
  last one always delivered, counters adding up) and a proptest against a single-slot model.
- No async runtime dependency: `recv` stores the waker and `post` wakes it, so the tokio
  socket task can await it.
