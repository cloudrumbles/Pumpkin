# Ironore backend

This fork maintains the Pumpkin backend used by Ironore singleplayer.

- Fork: https://github.com/cloudrumbles/Pumpkin
- Maintained branch: `main`
- Upstream: https://github.com/Pumpkin-MC/Pumpkin
- Upstream base: `eda69440e6369c5f8a05c3f69fc809c40dab93a6`
- Base release: `0.2.2+26.3-26.60`
- Java protocol: 777 (Minecraft 26.3)
- Pumpkin world DataVersion: 5023

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
- Load forced chunks through level tickets, so `/forceload` loads and
  generates them the way vanilla's forced tickets do.
- Size the overworld from `[terrain]` in `pumpkin.toml`: `generator` is
  `vanilla` (384 blocks) or `terrain_diffusion`, whose `scale` (1 to 6,
  default 6) sets the height, 2,096 blocks at scale 6. The overworld dimension
  type sent to clients is patched to match.
- Generate `terrain_diffusion` worlds from the regions ironore's
  pomme-terrain pre-generates into the world folder's `terrain-diffusion`
  directory (format in `generation/terrain_diffusion.rs`): the noise stage
  fills from each column's top, biomes come from the regions, and the surface
  estimate, structure height sampler and spawn search read the same tops.
- Pack heightmaps with as many bits as the world is tall; pump chunks keep the
  width in flag bits 5 and 6.
- Check world-placed features against the world's real height
  (`BlockAccessor::min_build_y` and `build_height`), not -64 and 384.
- `worldgen_bench` (pumpkin-core) generates 1,024 chunks through the real
  chunk system without a server, times each stage and hashes each chunk's
  parts against a golden file. Feature placement isn't deterministic across
  runs yet, so blocks, light and heightmaps differ between runs.
- Leave sky light above a column's surface to that chunk's own lighting pass,
  instead of flooding the open air of unlit neighbours one cell at a time.

Regression tests accompany these changes. A passing build or unit test does not
establish survival parity or compatibility with vanilla saves.

## Validation

Use an explicitly installed Rust toolchain supporting Rust 1.97 or newer
(upstream's MSRV since 0.2.2). On the development host the selected toolchain
is `nightly-2026-08-21`.

```sh
cargo +nightly-2026-08-21 test --locked -p pumpkin-command
cargo +nightly-2026-08-21 test --locked -p pumpkin-world world_clock_tests
cargo +nightly-2026-08-21 test --locked -p pumpkin-core --lib
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

## Qualification (2026-11-12)

The fork at the 0.2.2 merge, plus the one fix it needed, passed on Rust 1.99.1
stable, with `cargo check --workspace --tests --locked` clean:

- 89 `pumpkin-command` tests and five doctests.
- 326 `pumpkin-world` library tests, including the six clock regressions, the
  scheduler's foreground/background tests and upstream's priority test.
- 737 `pumpkin-core` library tests.
- `worldgen_bench` generated all 1,024 chunks through the merged scheduler
  (5.1 s on 8 cores). Its golden hashes predate upstream's worldgen changes, so
  they were not compared.

One `pumpkin-world` test fails on fork `main` too:
`write_world_info_reports_final_rename_failure` makes the world folder
read-only, which a test host with `CAP_DAC_OVERRIDE` ignores.

The merge needed one fix. Upstream's `populate_biomes` now matches on the biome
supplier, so the terrain-diffusion supplier got its own arm. That arm, and
terrain-diffusion generation in general, has no test yet. `qualify.py` and
native client acceptance were not run for this merge.

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
