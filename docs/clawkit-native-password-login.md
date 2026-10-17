# Native ClawKit account login

Version 3.19.75 adds username/phone/email and password entry in the remote account card, alongside browser SSO. Both use the same account and revocable remote session. Password login invokes the existing native command with username/password and uses `/api/codex-remote/sso/password`; passwords are cleared from the form and never persisted to the shared session file.

The native client validates account id/sub and session expiry before storing the token and refresh credential. Session renewal retains the login method, so password logout revokes the session without opening a browser. Browser SSO and its global logout continue to work. Login does not configure Codex models.

Validation: the account card tests cover form submission and browser entry independently; `clawkit_account::password` covers native HTTP requests, wrong credentials, session storage and identity mismatch. The backend real SQLite/ASGI tests cover aliases, disabled accounts, device limits, refresh and revocation.
