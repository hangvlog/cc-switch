# Mobile interactions 3.19.33

Expose original-owner clarification requests and async agent questions. Persist the mobile outbox in a private local SQLite file; drain while remote control is enabled, including when the mobile socket is disconnected. Never replace the owner's own queue or start a second app-server. Dispatch deduplication and unknown-outcome reconciliation use the existing ledger. Completed/cancelled entries erase queued text.

Build from the committed shell revision through the launcher release workflow. Rollback: original 3.19.31 installers. Native Android and live-owner acceptance are recorded in the parent repository's docs/front/clawkit-mobile-interactions-0.1.15.md.

Queues are namespaced by a hash of the authenticated account ID. Switching accounts stops the old worker and never drains the old account queue.
