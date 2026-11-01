# The September Smile salvage

`PROJECT_SEPTEMBER_SMILE/` was a planning campaign written 2026-09-20 to 2026-09-24 against the framework on `master` at `654c7183`: eight audits, their build handoffs, a 132-row build sequence and the findings F138 to F292 in `FRAMEWORK_GAPS.md`. The build was paused on 2026-10-01 and the framework was rebuilt on `experiment/di-redesign`. This folder holds the campaign's material, read item by item against the rebuild, so that what the audits found and decided is not lost.

The campaign was written against the old framework, and the rebuild changed course on purpose with a different architecture, so a status of superseded or dropped is the expected outcome for much of it. Nothing here is a build plan. Each item carries a status at the tip the reader checked, evidence, and, where the item still teaches something, the value written out with its provenance.

## Layout

| Path | Holds |
| --- | --- |
| `source/` | The campaign folder, copied unchanged (51 files). |
| `<subject>.md` | One record per subject, below. |
| `QUESTIONS_SALVAGE.md` | The questions that cross subjects, for the designer. |

ADRs 0052 to 0054, found on an unreachable commit (`9d3fb9c2`, the old HTTP audit branch), are in `docs/adr/` marked withdrawn.

The 25 old snapshot and backup branches were archived to `old-build-branches-2026-11-01.bundle` beside the repository, 5.5 MB, outside git history here.

## Status vocabulary

- **built**: the rebuild contains the item; the record cites where.
- **superseded**: the rebuild solved the same problem differently or removed what it concerned; the record names what replaced it.
- **open**: the defect or gap exists at the tip, or the question is unanswered.
- **dropped**: the concept is gone and nothing replaced it.
- **UNVERIFIED**: deciding needs a build or a run; the record says what.

## Records

| Subject | File | Items | built | superseded | open | dropped | unverified |
| --- | --- | --- | --- | --- | --- | --- | --- |
| DI | `di.md` | 227 | 115 | 55 | 7 | 41 | 9 |
| RPC | `rpc.md` | 89 | 41 | 32 | 9 | 5 | 2 |
| WebSocket | `ws.md` | 82 | 42 | 21 | 9 | 9 | 1 |
| gRPC | `grpc.md` | 80 | 32 | 29 | 8 | 8 | 2 |
| HTTP | `http.md` | 97 | 46 | 26 | 15 | 5 | 5 |
| Pipeline | `pipeline.md` | 81 | 34 | 27 | 10 | 9 | 1 |
| Extraction | `extraction.md` | 83 | 28 | 28 | 10 | 15 | 1 |
| GraphQL | `graphql.md` | 84 | 44 | 15 | 10 | 14 | 1 |
| Performance, render, campaign README | `performance-render-readme.md` | 83 | 21 | 34 | 15 | 8 | 5 |
| Build sequence (134 rows, 94 sequence-level items) | `build-sequence.md` | 228 | 93 | 91 | 16 | 23 | 5 |

The gRPC record counts one further item as method-only, and the extraction record one as another subject's pointer; the WebSocket row counts the two items built on one path and open on the other under built.

## Where the findings went

Each reader's live defects, and divergences the ledger did not hold, are filed in `FRAMEWORK_GAPS.md` as F524 to F561, by subject: gRPC F524 to F527, GraphQL F528 to F531, WebSocket F532 to F536, pipeline F537 to F541, extraction F542 to F546, RPC F547 to F551, HTTP and build sequence F552 to F557, DI F558 to F561. The performance and render record raised two further questions (the 504 for `ErrorKind::Timeout`, and the loss of an arbitrary gRPC status code), listed in `QUESTIONS_SALVAGE.md`.

## Reading order for the designer

1. `QUESTIONS_SALVAGE.md`, for the decisions that cross subjects.
2. Each record's "Worth discussing with the designer" section, for the decisions inside one subject.
3. A record's item table, for the evidence behind any verdict.
