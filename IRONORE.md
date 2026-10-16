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
- Process Java 26.3 accepted teleport poses through the movement handler, ignore
  superseded acknowledgements after completion, validate current poses, and wrap
  teleport IDs at the same boundary as vanilla.
- Parse block-entity NBT in `/setblock` and `/fill`. Apply it to the actual block
  entity with its type and position preserved, count NBT-only changes, and retain
  the data across restart.
- Match explicitly supplied properties and partial NBT in block and tag
  predicates for `/execute if/unless block`, `/fill replace`, and `/clone filtered`.
- Route fills through normal block placement, avoiding the `/fill destroy`
  read-lock deadlock, creating block entities, honoring strict mode, and reporting
  actual changes. Destroy mode permits drops; normal replacement suppresses block
  entity replacement side effects.

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

## Updating upstream

Preserve upstream history and review changes on a separate branch. Before
advancing Ironore's pin, check protocol and configuration changes, regenerate
artifact provenance, and repeat the affected command, persistence and real
client/server checks. Never open a user's only world copy during qualification.

## Remaining parity limits

Pumpkin owns a clock per World. Vanilla 26.3 has a server clock manager and
dimensions refer to shared clock IDs; this fork repairs persistence within
Pumpkin's existing architecture. The block command additions use Pumpkin's
existing block-entity serializers; unimplemented block-entity behavior and data
fields remain outside their scope. The Java reference declares world DataVersion
5023 while Pumpkin uses 4903, so vanilla world migration and full save compatibility
remain unqualified. Native client, survival, cross-dimension, and Bedrock
acceptance are not established by the minimal Java protocol harness. Other
gameplay gaps remain tracked upstream.
