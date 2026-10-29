# Proposal: split the decision log (not done yet)

**Status:** proposal (CI optimisation, item 5, 2026-10-23). It needs the owner's approval before anyone splits a file.

## The problem

From 2026-10-15 to 2026-10-22, these were the most-edited files on `dev` (non-merge commits):
- `docs/design/13-decision-log.md`: 74 commits. It has 2,380 lines and holds every D and Q row.
- `CHANGELOG.md`: 70.
- `docs/plans/README.md`: 43.
- `docs/design/README.md`: 42.

**Why it costs CI time:**
- Nearly every PR appends at the same place in these files. One merge into dev makes the other open PRs conflict.
- An agent then rebases and pushes, and every workflow of that PR runs again.
- With about 15 agents, one merge can trigger several full reruns.

**Not concentrated:** the new rows that link a design spread over about 15 designs. The most-linked was §75, with 7 of the 74 commits.

**The cheap fix:** `merge=union` on the decision log and the changelog (#1093).
- A local rebase now resolves these conflicts by itself.
- GitHub still reports the conflict, so the agent still has to rebase, push and rerun.
- So it cuts hand resolution, but not the reruns.

## The proposal

1. **One decision file per design.** Create `docs/design/decisions/NN-<slug>.md`, for example `75-loams-house-engine.md`. Each file has the same `| ID | Date | Decision | Rationale | Status |` table, plus that design's Q table.
   - Cross-cutting rows go to `00-general.md`.
   - The "Owner actions pending" sections go to `owner-actions.md`.
   - With concurrent PRs spread over about 15 designs, two PRs rarely edit the same file. A conflict then means two PRs really touched the same design.
2. **IDs don't change.** Numeric `D`/`Q` IDs stay global, and every existing reference stays valid.
   - `scripts/docs/check-decision-ids.sh` reads every file under `docs/design/decisions/`, and still fails a duplicate.
   - A PR is tested on its merge with the base branch, so a collision with merged rows is caught on the PR.
   - To make collisions between two open PRs rare, `scripts/docs/next-id.sh` hands out the next free ID from `origin/dev`.
3. **A generated index, not a hand-maintained one.** `scripts/docs/decision-index.py` writes the index: ID, date, the first sentence, status, and a link to the file.
   - **Recommended:** don't commit the index. A committed generated index would be one more file that every PR edits.
   - `docs/design/13-decision-log.md` becomes a short page explaining where the decisions live and how to find one (`scripts/docs/decision-index.py --find D1575`, or `rg '^\| D1575 '`).
   - loams-site's docs build renders the full index.
   - **Owner alternatives:** commit the index and regenerate it on a nightly bot commit (this needs a bypass of "dev: team updates only"), or commit it with `merge=union` and a CI check that it is up to date.
4. **The same pattern for the two README indexes.**
   - Each plan gets front matter: `status`, `milestone` and `design`. `docs/plans/README.md` is then generated from it the same way.
   - `docs/design/README.md` keeps its hand-written reading order, glossary and conventions. Only its document table is generated from each design's first heading.
5. **Generated files** (`docs/house/compat-matrix.md`, `docs/house/compat-profile.md`, the SDK facades): keep them committed, since the `the_committed_matrix_is_up_to_date` and golden tests guard them.
   - Add `scripts/regen-generated.sh`, which rebuilds all of them, and tell agents to run it after every rebase.
   - In the last week, 5 failures of `the_committed_matrix_is_up_to_date` and 5 of the facade goldens were stale files after a rebase.

## The migration (one PR, after approval)

1. `scripts/docs/split-decision-log.py` moves each row to the file of the design it links first (`[§NN]`). Rows with no link go to `00-general.md`. The order inside each table stays the same.
2. `check-decision-ids.sh` and its self-test read the new tree.
3. The PR lands at a quiet time. Open PRs that add rows then rebase once: their new rows go to the matching design file, and the script can do that for them.

## What the owner decides

- Split by design (recommended), or by ID range. Ranges of 100 IDs fill in about two days at today's rate, so concurrent PRs would still collide.
- Where the index lives: not committed and rendered by the site (recommended), a nightly bot commit, or committed with `merge=union` and an up-to-date check.
- Whether `docs/plans/README.md` moves to generated front matter in the same change.
