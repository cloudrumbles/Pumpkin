# Ironore backend

This fork maintains the Pumpkin backend used by Ironore singleplayer.

- Fork: https://github.com/cloudrumbles/Pumpkin
- Maintained branch: `main`
- Upstream: https://github.com/Pumpkin-MC/Pumpkin
- Upstream base: `204a94ed895f041a845630d5a23c094705a0704e`
- Base release: `0.2.0+26.3-26.51`
- Java protocol: 777 (Minecraft 26.3)
- Pumpkin world DataVersion: 4903

Ironore records the exact supported fork commit in
`tools/pumpkin-backend.json`. Locally built artifacts carry their own executable
hash and build provenance. Publisher checksums apply only to upstream publisher
artifacts.

## Changes

- Persist runtime clock values through save, shutdown, unload and restart.
- Ignore superseded teleport acknowledgements while retaining the current request.
- Remove both live and serialized block-entity records when a container is removed.
- Read and save `NoGravity`, and honor `NoAI` for bat and generic mob decisions.
- Parse namespaced function IDs and validated block-state properties for
  `/setblock` and `/fill`.
- Merge repeated command branches so entity and block `/data` queries remain
  reachable alongside storage queries.

Regression tests accompany these changes. A passing build or unit test does not
establish survival parity or compatibility with vanilla saves.

## Validation

Use an explicitly installed Rust toolchain supporting Rust 1.96 or newer. On the
development host the selected toolchain is `nightly-2026-08-21`.

```sh
cargo +nightly-2026-08-21 test --locked -p pumpkin-command
cargo +nightly-2026-08-21 test --locked -p pumpkin-world world_clock_tests
cargo +nightly-2026-08-21 test --locked -p pumpkin --lib
```

The generated data crate needs a larger compiler stack on this toolchain. The
host qualification uses `RUST_MIN_STACK=33554432`, one Cargo job, no debug info,
no incremental compilation and 256 development codegen units. These are QA
build settings; use a separately measured release build for performance claims.

The local Minecraft 26.3 Java reference is derived from the named game JAR.
The official server JAR used for runtime comparison has SHA-256
`d052f14d7a173734fba553711e5b570162e2f2a313267ee31a21b975a679be64`.
Keep Java source, game assets, test saves and executable artifacts outside git.
Use disposable worlds for save/reopen and protocol tests.

## Paused qualification checkpoint (2026-10-15)

Source `2ad2dc055` passed 85 command tests and five doctests, including three
regressions that failed before the repeated-branch repair. The corrected
qualification harness passed 17 Python tests, strict typing and lint checks,
and all 22 system checks against official vanilla 26.3.

The earlier fork revision `987e8de99` passed 344 server library tests and six
clock regressions. Its staged development executable retained frozen time 6000
across restart and kept the protocol client connected through queued old/new
teleport acknowledgements. That runtime run failed overall; fixture setup and
unsupported observations prevented qualification of the remaining behaviors.

The latest source build and queued server-test rerun were stopped at the user's
request. Current-pin runtime and native client acceptance remain pending.

## Paused 0.2.1 sync (2026-10-26)

Branch `ironore/upstream-sync-20261026` merges upstream 0.2.1+26.3-26.51
(`224f9f8144ca53a8d0800acb87607d714196e2a6`) into main at `f7e637be7`. The
merge commit's message lists how each conflict was resolved. Main has not
moved, and the header above still describes the 0.2.0 base until the steps
below pass.

Nothing on this branch has been built or tested. The first cold development
build ran out of space before it finished. It was run with 4 jobs,
`CARGO_PROFILE_DEV_DEBUG=0` and incremental compilation off, with its target
directory inside this worktree. On the development host, `~/workspace`,
`~/.cache`, `~/.cargo` and `/var/tmp` share one 127 GB ext4 partition, which
was already nearly full. The build was stopped and its target deleted, but the
partition still had only 1.2 GB free afterwards. The target's size was not
measured. Before building again, free space on that partition or put the
target directory on `/home/shah`, which is a separate, larger btrfs partition.

To finish the sync:

1. Build: `RUST_MIN_STACK=33554432 cargo +nightly-2026-08-21 build --locked -p pumpkin --bin pumpkin`.
2. Test. The server library moved from `pumpkin` to `pumpkin-core`:

   ```sh
   cargo +nightly-2026-08-21 test --locked -p pumpkin-command
   cargo +nightly-2026-08-21 test --locked -p pumpkin-world world_clock_tests
   cargo +nightly-2026-08-21 test --locked -p pumpkin-core --lib
   ```

   The fork's regressions to confirm are the function-ID tests in
   `commands/function.rs`, `no_ai_stops_bat_decisions_but_keeps_roosting_physics`
   in `entity/living.rs`, `no_gravity_round_trips_through_base_entity_nbt` in
   `entity/mod.rs`, the block-entity tests in `world/mod.rs`, the clock tests
   in `world/time.rs` and the teleport-acknowledgement tests in
   `confirm_teleport/state.rs`.
3. Run `tools/ironore/qualify.py --backend pumpkin` against the built
   executable, as `tools/ironore/README.md` describes.
4. Update the header above: upstream base `224f9f81`, base release
   `0.2.1+26.3-26.51`, DataVersion 5023 and the validation commands. Replace
   this section with the qualification result.
5. Fast-forward `main` to the branch and push. Never force-push.
6. In Ironore, set `tools/pumpkin-backend.json` to the new `fork_commit`, with
   `upstream_base` `224f9f81…`, `release` `0.2.1+26.3-26.51` and
   `world_data_version` 5023, then run `tools/test_build_pumpkin.py`.

The worktree for the earlier, stalled sync to `9c04f714` (branch
`ironore/upstream-sync-20261023`, an uncommitted merge) was left untouched.
This branch supersedes it.

## Updating upstream

Preserve upstream history and review changes on a separate branch. Before
advancing Ironore's pin, check protocol and configuration changes, regenerate
artifact provenance, and repeat the affected command, persistence and real
client/server checks. Never open a user's only world copy during qualification.

## Remaining parity limits

Pumpkin owns a clock per World. Vanilla 26.3 has a server clock manager and
dimensions refer to shared clock IDs; this fork repairs persistence within
Pumpkin's existing architecture. Block-state command parsing does not add
block-entity NBT arguments or complete block-predicate matching. Teleport
acknowledgement handling does not yet validate the 26.3 accepted pose. Other
upstream gameplay gaps remain tracked upstream.
