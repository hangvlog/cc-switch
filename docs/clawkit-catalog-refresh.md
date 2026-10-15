# Conversation list refresh lanes

The remote relay previously held the command Bridge mutex for every RPC. A lightweight 20-row directory read that took 262 ms alone took 1606–1693 ms when requested together with a passive status scan and activity synchronization.

`CatalogReader` now serves only `desktop/thread/list` and `desktop/thread/statuses` outside that mutex, using the same catalog validation and owner status implementation. Each lane allows two in-flight reads. Other methods, queue draining, dispatch receipts and task mutations keep their existing serialization. Disabling remote access also disables cloned catalog readers. No new credentials, caches, status inference or protocol are introduced.

Validation: the catalog reader test holds the command bridge mutex while another thread reads a real SQLite catalog and verifies identity, content, invalid cursor handling, method allowlist and disable behavior. Production acceptance must repeat concurrent directory/status/activity requests against the installed application. Mobile 0.1.50 is compatible without an APK update.
