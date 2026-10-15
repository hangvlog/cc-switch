# Remote image previews

Desktop 3.19.67 advertises `imagePreview`. Existing `desktop/image/read` accepts `variant: thumbnail` and optional `chunkSize` (multiple of three, up to 196608 bytes). Defaults preserve original 49152-byte reads. Responses include variant and chunkSize. Account-scoped attachments generate a private cached 480px JPEG preview using bounded decoder allocation; source bytes are unchanged. Mobile 0.1.51 downloads previews first and originals only on expansion, with at most three global image requests.

Validation: preview dimensions, original byte preservation and cache hit test, existing attachment scope/retry test, mobile ordered chunk assembly and malformed response test.
