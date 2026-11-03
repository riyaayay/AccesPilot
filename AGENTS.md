# AccessPilot rules
- Money is i64 minor units + a Currency enum. Never use floats for money.
- crates/ap-core: no I/O, no async, no unwrap/expect/panic, no `as` casts on money. Pure functions and enums only.
- Every lifecycle (purchase, reservation, grant, policy) is an enum with a total transition function.
- Never hold a database transaction open across a network call.
- A state change and its ledger event are written in the same transaction.
- Unknown input, unknown status, unknown price => hold money, withhold content, ask a human. Never default to Allow.
- Write the failing test first. Before finishing any task run: cargo fmt && cargo clippy -- -D warnings && cargo test
- Do not guess Hyperswitch field names or headers. Check https://api-reference.hyperswitch.io; if unsure, add a TODO and a failing test.
- Secrets come from environment variables. Never print or commit them.
- Do not mark a task done until its acceptance test passes and you have shown the output.

