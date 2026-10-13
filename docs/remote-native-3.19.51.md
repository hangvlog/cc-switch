# Remote native follow-up 3.19.51

Windows read-state uses the installed official desktop IPC named pipe, with server SID validation, bounded reads, and a default-profile guard because the pipe is not CODEX_HOME scoped. The read command still waits for the official persisted state to acknowledge the broadcast; a successful write is not treated as acceptance. A read-only test initialized the live Windows official IPC; end-to-end GUI read acknowledgement is tracked separately.

Engine completion/attention reminders retain at most 100 metadata records in the private engine runtime directory. Records survive engine restarts and are partitioned by a digest of the local authentication file; no credentials or message bodies are stored. Authentication changes conservatively start a new bucket. Storage failures do not stop the original engine.

Verification: macOS 34 bridge tests and installed-engine synthetic E2E pass, including reminder recovery after engine restart. Windows IPC initialization and isolated-profile rejection pass with the installed official desktop. Packaging and release evidence is maintained by the parent chat-all repository.
