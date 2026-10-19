# Ironore backend

This fork maintains the Pumpkin backend used by Ironore singleplayer.

- Fork: https://github.com/cloudrumbles/Pumpkin
- Maintained branch: `main`
- Entity AI work branch: `ironore/vanilla-parity-20261017` (not merged)
- Upstream: https://github.com/Pumpkin-MC/Pumpkin
- Upstream base: `204a94ed895f041a845630d5a23c094705a0704e`
- Base release: `0.2.0+26.3-26.51`
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
cargo +nightly-2026-08-21 test --locked --lib -p pumpkin-world
cargo +nightly-2026-08-21 test --locked -p pumpkin --lib
```

The generated data crate needs a larger compiler stack on this toolchain. The
host qualification uses `RUST_MIN_STACK=33554432`, one Cargo job, no debug info,
no incremental compilation and 256 development codegen units. These are QA
build settings; use a separately measured release build for performance claims.

The local Minecraft 26.3 Java reference is derived from the named game JAR.
The official server JAR used as a reference has SHA-256
`d052f14d7a173734fba553711e5b570162e2f2a313267ee31a21b975a679be64`.
Keep Java source, game assets, test saves and executable artifacts outside git.
Use disposable worlds for save/reopen and protocol tests.

## Entity AI continuation (2026-10-18)

The entity port targets the stable Java 26.3 server: protocol 777 and DataVersion
5023. Its official server JAR has SHA-1
`33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c`. The version constant is shared by world,
chunk, POI, player and structure persistence.

The current work includes neutral-mob anger and melee callbacks, aquatic and
flying behavior, and entity-specific movement controllers. Navigation now uses
each mob's collision dimensions, forwards wanted positions to its controller,
and activates paths supplied by melee goals. The path search excludes visited
nodes so it cannot repeatedly rebuild cyclic predecessor chains.

`tools/ironore/entity-parity-26.3.json` records the Java class hierarchy and
declared goals for all 90 mob types, alongside the 71 other registered entity
types. It is a requirements inventory, not a parity score. Brain activities,
species-specific states, interactions, lifecycle behavior, persistence and
exceptional conditions still require implementation and qualification.

`tools/ironore/qualify_entities.py` starts a real server in a disposable world,
connects a protocol-777 client, advances by observed game ticks, checks entity NBT
and metadata, and records a clean shutdown. Positive movement checks cover a
provoked zombie routing around a wall, cod swimming and allay flight. NoAI
stationary checks are separate requirements; they do not establish active AI
parity. Compact run records are kept in `tools/ironore/reports/`; full command
transcripts and test worlds remain with the qualification runs outside git.

The movement executable SHA-256
`64b253140d6a7b02f886f2254573bf80c93d8d0c31c0ae98e51719d6edb09b7a`
passed 114/114 live checks on October 18: 90 NoAI checks, 23 active behavior
checks and clean shutdown. The official Java reference passed the original
111-check suite and the separate three-probe movement suite with clean shutdown.
These results qualify the listed scenarios, not every behavior of each entity.

### 2026-10-19 entity continuation

The official Java 26.3 server reference has been checked for aging and age-lock
transitions, cow breeding and parent cooldown, rabbit jumping, and mature-carrot
raiding. The expanded 181-case Java reference run passed 180 scenarios. One
free-swimming pufferfish left its threat radius before fully inflating; the
corrected restrained pufferfish fixture passed. Rabbit pursuit and the revised
bounded-garden carrot-raiding fixtures also passed against vanilla.
Compact reports are in tools/ironore/reports, preserving failing fixture
observations rather than silently counting them as passes.

This branch adds rabbit hopping, carrot raiding, variants and coat inheritance;
fixes age ticking for cow, pig, sheep, wolf and chicken; sets vanilla-style
breeding parent ages; applies the ageable feeding API; and fixes Java 26.3
interaction packets so the correct hand is used.

The Rust cargo-check gate passed with a larger compiler stack and one Cargo
job. This is a source/type check, not proof that the newly changed executable
passed live Pumpkin behavior tests. Full entity-by-entity vanilla AI parity
is still not established.

This branch does not yet establish 100% entity AI parity and does not change
Ironore's supported backend pin.

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

## Qualification continuation (2026-10-16)

The paused runtime work resumed from fork commit
`f7e637be76ff256ac5eeb157d4848bfa381af602`. The corrected original harness passed
all 22 checks against that baseline. New regression suites exposed discarded
accepted teleport poses, incorrect acknowledgement validation, missing block NBT
and predicate behavior, and a `/fill destroy` deadlock. These are repaired on
`ironore/vanilla-parity-20261017`.

The final executable was built from clean source
`eebbb01de1e40906c4891e10650e0c7823a565fe` with the pinned toolchain and settings
above. Its SHA-256 is
`6258f2c10df62754665c3fdfd39fca2e799c27949ec81a66e886957ae00b6d8e`.
The existing dependency pins and `Cargo.lock` were preserved.

| Validation | Result |
| --- | --- |
| Pumpkin server library tests | 354 passed |
| Command tests and doctests | 93 tests and 5 doctests passed |
| World library tests, including clock regressions | 231 passed |
| Python harness tests | 20 passed |
| Original command, protocol and restart suite | 22 checks passed |
| Teleport acknowledgement suite | 12 checks passed |
| Block input, predicate and restart suite | 15 checks passed |
| Rust formatting and all-target Clippy for the three affected crates | Passed |
| Strict mypy for the four harness files and Ruff | Passed |

All 49 runtime checks used the same final executable in disposable worlds. The
full results, commands, baseline observations and build provenance are recorded
in [the qualification report](tools/ironore/results/java-26.3-20261016.json).
The [harness README](tools/ironore/README.md) documents how to repeat each suite.
Cargo still emits an existing future-incompatibility notice for
`proc-macro-error2 v2.0.1`; Clippy has no remaining diagnostics in the affected
crates. The test and lint results cover the listed packages and default features.

This continuation inspected official Java 26.3 bytecode and ran Pumpkin; it did
not launch a vanilla server or accept the Minecraft EULA. The historical vanilla
run above remains separate evidence. Native client acceptance and the remaining
parity limits below are still open. This qualification does not advance the
external Ironore backend pin or establish completion of the full vanilla port.

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
5023, now also written by this branch, but vanilla world migration and full
cross-server save compatibility remain unqualified. Native client, survival, cross-dimension, and Bedrock
acceptance are not established by the minimal Java protocol harness. Other
gameplay gaps remain tracked upstream.
