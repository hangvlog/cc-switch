# Latest-message remote reads

Desktop 3.19.65 adds optional `latestOnly` to thread history/read/subscribe and owner watches. It preserves current runtime, active turns, pending human requests and control capabilities while returning only the latest visible user/assistant message. Internal send reconciliation retains its existing full snapshot. Mobile expands older messages through saved-history paging so messages earlier in the same turn remain available.

`desktop/thread/list` supports a bounded `limit` (1–50) and `lightweight=true`, which omits transcript summaries and bounds preview text. Status queries return summaries for explicitly requested rows. Legacy callers retain the original defaults. Source tests cover bounded catalog paging, saved-history continuation and active-state retention.
