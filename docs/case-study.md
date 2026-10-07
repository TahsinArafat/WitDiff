# Case study — what WitDiff contributes, measured

A controlled comparison: the same coding agent, the same model, the same task,
on the same buggy repository, run with and without WitDiff installed. Repeated
with a stronger prompt. Judged by reading the diffs and running the code, not by
either arm's own report.

This document exists because WitDiff makes a claim that deserves evidence rather
than assertion, and because a claim that cannot be measured is not worth
publishing. The headline result is a null one.

---

## Setup

**Subject.** [`Sakib20050413/ICT_Fest_Hackathon_Preliminary`](https://github.com/Sakib20050413/ICT_Fest_Hackathon_Preliminary) —
a FastAPI coworking-space booking API written for a hackathon bug-fixing
competition. `README.md` states the required behaviour as sixteen numbered
business rules; the code contradicts many of them. It has one commit and no fix
history, so **there is no published ground truth** — "correct" below means
consistent with the specification as a reader understands it.

| | |
|---|---|
| Agent | Pi 1.0.3, non-interactive |
| Model | `deepseek-v4.1-flash:free` |
| Control arm | Pi with a config containing no WitDiff package |
| Treatment arm | Identical, plus WitDiff installed and configured for the repository |
| Isolation check | Asked whether it had any witdiff skill or tool, the control answered `NONE` |

The two prompts differ by exactly one section — `## Verifying your work` — plus
one bullet in the required report format. `diff` produces two hunks; every other
byte is identical.

---

## Headline: WitDiff did not find more bugs

| | Control | Treatment |
|---|---|---|
| Defects claimed, round 1 | 21 | 18 |
| Defects claimed, round 2 | **29** | **29** |
| Round-2 cross-check | — | treatment's tests fail 1 of 61 against control's code |

Four independent runs. The defect counts are level.

**Any claim that a verifier makes an agent find more bugs is not supported by
this experiment, and WitDiff's own documentation does not make it.** The value
claimed elsewhere in this repository is narrower — see
[What it contributes](#what-it-contributes).

---

## Behaviour: four observable differences

### 1. It committed progressively

| | Control | Treatment |
|---|---|---|
| Round 1 commits | 1 | 4 |
| Round 2 commits | 1 | 5 |

Control committed once, at the end. Treatment committed as it worked:

```
428d9b9  Fix business-rule violations in datetimes, bookings, auth, export, stats
6ff409d  Add regression tests for audited business rules
0452c08  Add malformed datetime regression test
7f3a7c8  Add booking-detail start_time regression test
413b6d7  Add audit report
```

Five commits, none of them mine; the baseline commit that configured WitDiff
excluded from every count in this table.

This is the difference that makes every other difference possible. A red/green
proof compares **two real revisions**. One commit means no proof is not weaker
but *absent* — the comparison cannot be constructed at all.

### 2. It acted on a negative verdict

Round 1, from receipts the agent committed itself:

```
605c4f6  "Fix business-rule violations and add regression tests"
         status: no_changed_tests   reason: no_dedicated_tests_changed

c7e97b5  "Strengthen regression tests for past-start and zero-duration"
         status: verified           reason: proven
```

It did not accept the green suite, did not rationalise, and did not report
`COMPLETE` anyway. **It went back and rewrote two tests, then re-checked.**

A passing test run looks identical whether the tests are regression tests or
not. Without a signal that distinguishes them, there is nothing to act on.

### 3. It cited checkable evidence

Treatment, closing statement:

> verified `status: verified`, `reason: proven`, `red_green_proven: true`, with
> 24 of my added tests failing on the base revision

Control, closing statement:

> all covered by tests that fail on the unmodified code

Both asserted the same thing. Only one could point at a receipt. The claim is
reproducible in the first case — `git show <commit>:.witdiff/receipt.json`
returns the evidence — and checkable against the repository in the second.

### 4. The treatment caught a live defect the control shipped

Probed directly, not read from a report:

| | Malformed datetime |
|---|---|
| Pristine base | `500 Internal Server Error` |
| **Control** | **`500 Internal Server Error`** |
| **Treatment** | `400 {"code": "INVALID_BOOKING_WINDOW"}`** |

The specification names 400 for invalid booking input. The control's report says
`STATUS: COMPLETE` and does not mention it. The cross-check makes the asymmetry
measurable: the treatment's test suite passes 63 of 63 against the control's
code; the control's suite fails 1 of 61 against the treatment's code.

Note the mechanism: this is not WitDiff *finding* the defect. It is the treatment
agent's own audit finding it, plus a commit structure that let its tests run
against the pristine base and surface it.

Those cross-check figures are from **round 2**, whose suites hold 63 and 61
tests. The round-1 suites, at 22 and 17 tests, were not cross-checked this way;
the round-1 credibility figures below are a different measurement.

---

## Test credibility, independently measured

Each arm's final state was re-verified against the **pristine** commit rather
than trusting either report.

Round 1, where both suites were small enough to complete the comparison:

| | Control | Treatment |
|---|---|---|
| `status` / `reason` | `verified` / `proven` | `verified` / `proven` |
| `red_green_proven` | `true` | `true` |
| Tests failing on base | 16 | 15 |

Round 2 could not be measured this way for the control. Its suite grew to 63
tests including concurrency cases that do not terminate against the unfixed
code — each file runs in about 0.1s alone, while the suite together ran past 60
minutes. WitDiff reported `base_experiment_timed_out` and declined to issue a
verdict, which is correct behaviour: a suite that does not finish cannot
demonstrate a behavioural regression. **So no round-2 credibility figure is
claimed for the control.** The treatment's round-2 check completed:
`verified` / `proven`.

**The control also produced credible regression tests, without WitDiff.** A
capable agent given a written specification already writes tests that fail on
the buggy revision. This is the strongest evidence against a large effect and
the reason the null result leads this document.

One measured limitation: the control's suite includes concurrency tests that do
not terminate against the unfixed code — each file runs in ~0.1s alone, while
the suite together ran past 60 minutes. WitDiff reported `base_experiment_timed_out`
and declined to issue a verdict, which is correct: a suite that does not finish
cannot demonstrate a behavioural regression. Its 16/0 figures therefore cover
its non-concurrency tests.

---

## Where the control was better

Reported because a case study that only lists wins is not a case study.

**Rule 14.** The specification requires room statistics to *"always equal the
values derivable from the bookings themselves."* The base keeps counts in a
dictionary with a deliberate `sleep(0.1)` between read and write, so concurrent
bookings lose increments.

- Control deleted the module and derived from the database; removed the callers.
- Treatment left `app/services/stats.py` **byte-identical to the base**, fixed
  only the `rooms.py` call site, and still calls the racy `record_create` /
  `record_cancel` on every booking. Nothing reads the result.

User-visible behaviour is identical. The residue is a live unsynchronised write
path with a misleading docstring.

WitDiff contributed nothing here, in either direction: `app/services/stats.py`
is production code, matches none of the configured `test_globs`, and this project
does not analyse production code outside Rust mutation. The receipt correctly
carried zero integrity findings — there was nothing for it to see.

Neither arm was clean, and both self-reported `STATUS: COMPLETE`, so that line
discriminates nothing between them.

---

## The prompt dominated the result

The first prompt was judged too weak and both arms re-run. The strongest effect
in the whole experiment was not the tool:

| | Weak prompt | Exhaustive prompt |
|---|---|---|
| Control defects | 21 | **29** |
| Treatment defects | 18 | **29** |

Requiring a per-rule audit table, systematic boundary checks, cross-path
symmetry review, and attention to the deliberate race-window hooks nearly doubled
both arms' output. **That effect is larger than anything WitDiff contributed.**

---

## What it contributes

1. **It makes verification possible.** A red/green proof needs two revisions.
   An agent that commits once cannot be checked at all.
2. **It gives an agent a reason to distrust a green suite.** When the receipt says
   `no_changed_tests`, the agent strengthened two tests and re-checked — behaviour
   with no external trigger.
3. **It turns an assertion into evidence.** A self-report of "my tests fail on the
   original code" is a claim; a committed receipt is checkable.
4. **It makes its own work auditable.** The receipt lives in the repository, so
   a third party can re-run the claim rather than believe it.

## What it does not contribute

1. **Finding bugs.** Level, across four runs.
2. **Writing better code.** Each arm shipped something the other missed.
3. **Judgement about the change.** `verified` says a test distinguishes two
   revisions; it does not say the change is correct. See
   [verification-model.md](verification-model.md).
4. **Any publishable rate.** One repository, one model, four runs, no ground
   truth. A benchmark against real agent-generated pull requests is still open —
   see [roadmap.md](roadmap.md), M9.

---

## Reproducing this

Artefacts, including both prompts verbatim, the agent configuration for each arm
with the API key redacted, full agent stdout, the committed receipts, and the
diff-by-diff judgement, are preserved outside the repository under
`/tmp/bench-work/`:

```
repo0/          pristine clone, never modified
repo1/ repo2/   the two arms, committed
round1/         first run: prompts, logs, report
record/
  prompt-control.md / prompt-witdiff.md   the only difference is the WitDiff section
  control/ witdiff/   settings.json + models.json, key redacted
  run*.log                                full agent stdout
  JUDGEMENT.md                            diff-by-diff findings
```

Methodological notes, recorded so the numbers can be read correctly:

- The **first control attempt was interrupted** and its partial state is kept
  separately; the reported control result is from a clean restart.
- Judging was done by **reading diffs and probing endpoints**, not by trusting
  either arm's report. Where a report was wrong, this document says so.
- No score is assigned here. The repository's authors have not published the
  official tests, so correctness is left to independent review.

---

## Summary

WitDiff did not find more bugs. It changed what an agent's work could be, and
what the agent did when its tests turned out to be worthless.

The clearest evidence is a commit message — *Strengthen regression tests* —
written by an agent that had been told its tests proved nothing, in a run where a
comparable agent had no way to learn that.
