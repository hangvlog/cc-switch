# Mobile image echo — Desktop 3.19.42

Original Codex user-message history stores phone uploads as `local_image` paths. The bridge previously flattened user content to text and discarded this variant; the phone also lacked a message image renderer.

History and live projections now return `{type:"image",imageId}` only for immutable PNG/JPEG files inside the connected account attachment directory. Paths are never returned or accepted by the read RPC. `desktop/image/read` accepts a 64-character image ID and offset, returns at most 49,152 bytes per call, and rejects unknown IDs, symlinks, invalid offsets and files exceeding 5 MiB. Other images retain an unavailable placeholder.

Mobile 0.1.22 displays thumbnails and an image viewer and caches under server/account/desktop scope. Existing stored phone uploads work after desktop update; no text matching or resending is used. New conversation and queue dispatch retain existing receipt semantics.

Validation: attachment account isolation/traversal/symlink/offset cases; saved-history and live-history tests; real current-thread three-image round trip with SHA-256 equality; isolated original Codex engine image upload → native project creation → history → image read → engine restart, with request deduplication. Synthetic model used only in the isolated send test.

Release builds use the source commit on Mac and home Windows HANG, then direct own-server publication. This version can be installed manually. The original Tauri update private key is unavailable, so automatic-update artifacts are not generated or registered; the configured public key remains unchanged. Prior installer 3.19.40 is the rollback.
