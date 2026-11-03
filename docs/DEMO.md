# AccessPilot — 3-Minute Verification Demo Script

This script walks through the three-minute verification demo using the AccessPilot Dashboard and API. Every beat ends on an artifact a reviewer can directly verify: a ledger row, an atomic counter, a tamper badge, or a printed benchmark table.

---

## Demo Script Walk-Through

| Timestamp | Beat | Action | Verifiable Outcome |
|---|---|---|---|
| **0:00** | **Set the rules** | Open dashboard (`http://localhost:8080/dashboard`). View task form and Policy JSON. | Budget meter explicitly reads: **"₹50.00 limit (not a balance)"**, with 0 reserved and 0 captured. |
| **0:20** | **Free resource** | Agent fetches metadata from Provider A (`GET /v1/resources/provider_a/meta`). | HTTP 200 returned immediately. No transaction is created; budget stays ₹0.00. |
| **0:35** | **Two purchases** | Click **"Run Agent Demo"** (executes Provider B for ₹10.00 and Provider C for ₹15.00). | Ledger timeline appends `reserved`, `provider_result`, `captured`, and `grant_issued`. Budget displays: **₹25.00 captured, ₹25.00 remaining**. |
| **1:00** | **Blocked purchase** | Agent attempts to purchase Provider D resource for ₹40.00. | Policy engine halts execution with `Verdict::Deny`: **"expected total ₹40.00 exceeds remaining budget ₹25.00"**. Zero provider calls made; budget untouched. |
| **1:20** | **Idempotency storm** | Click **"Fire 100-Request Storm"** (fires 100 concurrent requests with the identical idempotency key). | Simulated PSP charge counter increments by exactly **1**. Replay counter logs **99 replays**. Zero duplicate charges. |
| **1:45** | **Crash recovery** | Click **"Simulate Mid-Purchase Crash"** then click **"Run Reconciler"**. | Purchase recovered from stranded reservation; settled into `CAPTURED`. Budget cap never breached. |
| **2:10** | **Fail-closed on unknown** | Attempt purchase against flaky Provider E with timeout injection. | Purchase status immediately sets to `PENDING_UNKNOWN`. Reservation held; content grant strictly withheld. |
| **2:30** | **Tamper detection** | Click **"Simulate Ledger Tamper"** (or modify a row in `psql`). | Dashboard ledger badge immediately flips from green **"INTACT"** to red **"TAMPER DETECTED AT SEQ #3"**. |
| **2:45** | **Verified numbers** | Run `make bench` in terminal to display the 10-test verification table. | All 10 tests display real measured hardware metrics and passes. |

---

> **Close Line**: *"Every rupee and every decision is explainable, and the system fails safe when it does not know."*

