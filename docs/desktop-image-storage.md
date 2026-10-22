# Desktop image storage

The remote bridge retains original images for message history and upload retry parts. A fixed 256 MiB ceiling blocked every new image once history accumulated, even with free disk space. Version 3.19.76 raises the default account storage limit to 2048 MiB and reads an optional account-local `image-storage.json` on every write:

```json
{"maxStorageMiB": 2048}
```

Place it beside the account `images/` directory (under `desktop-remote/accounts/<account scope>/`). Supported range: 256–65536 MiB. Invalid configuration fails explicitly. Raising the limit takes effect without restarting. The limit includes originals, previews and retry parts; finalization reserves the immutable copy before accepting the last chunk. Existing originals are never evicted. The 5 MiB per-image limit and account isolation are unchanged.

For recovery, completed `.part` files older than the retry window can be moved to a private backup only after verifying their SHA-256 matches an existing original. Do not delete original images to recover capacity.

Validation: reproduced the original cache-full error with 256 MiB of retained history; verified upload/read/retry after the fix, live configuration reload, final-copy capacity reservation and invalid configuration handling.
