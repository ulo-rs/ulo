# #185 — test: make the startup guards able to fail

Merged 2026-08-24 into `master` from `test/startup-guards-can-fail`, commit [`24c6566`](https://github.com/ulo-rs/ulo/commit/24c65663af4a7dab24930e3071a2957da9042744).

The elapsed-time assertions guarding the database startup checks were written as constants, and two of them sat at thirty seconds against a schedule whose worst case is 850ms. A guard that loose cannot fail for the reason it exists.

The redis guard admitted the exact regression the check exists to displace: with its internal retry re-enabled — six attempts with its own exponential backoff — the committed test **passed, in 18.91 seconds**. Diesel asserted no timing at all.

The bounds now derive from the schedule under test: `worst_case()` for the upper one, `retry_delay()` for the lower. Widening the policy widens the guard, so a constant cannot drift away from the thing it guards.

### What each guard does against its own regression

| regression | result |
| --- | --- |
| redis — internal retry re-enabled | fails, 18.91s |
| seaorm — driver timeouts not set | fails, 60.06s |
| sqlx — acquire timeout not set | fails, 60.07s |
| mongodb — server-selection timeout not set | fails, 60.06s |
| diesel — retry removed, probes once | fails, on the lower bound |

Every one fails, and each for its own reason rather than incidentally. The upper bound catches a driver waiting on its own timeout; the lower catches the retry being lost, which is the failure mode for the two drivers whose attempts are refused instantly.

Test-only. No library change.
