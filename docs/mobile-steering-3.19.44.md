# Mobile active-turn dispatch 3.19.44

`desktop/turn/start` now selects the original owner's `thread-follower-steer-turn` v1 while its current turn is active, and the existing start-turn v2 while idle. No second engine or interruption is introduced. The owner executes `turn/steer`; a successful response must contain the accepted turn ID, not merely an IPC acknowledgement. Unknown delivery remains durable and is never resent automatically. Blocking requests, owner changes, permissions and settings boundaries are preserved.

Live snapshots expose `activeTurnId` and `canSteer`; initialize exposes `messageSteering`. Async clarification answers use the same dispatch path. Existing account-scoped outbox entries drain through steering when active; pending synchronous questions/approvals still pause dispatch. New mobile messages no longer enter the idle-only queue.

Focused IPC and clarification tests pass. An isolated installed official Codex engine accepted a steer while a synthetic model response was held, and its next model request contained the added text before completion; repeat dispatch returned the same receipt with one send. This checks engine behavior through a test IPC owner, not a real phone or desktop GUI owner. Release evidence in the parent repository records further acceptance and package identity. Rollback: 3.19.43 manual installers.
