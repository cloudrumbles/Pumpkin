# Ironore Java 26.3 qualification harness

This harness starts one server that it owns in a brand-new disposable directory,
connects a minimal loopback Java protocol 777 client, runs console fixtures, stops
the server, inspects persisted NBT, restarts it, and checks the saved state. Every
check is written to `qualification.json` as `pass`, `fail`, `unsupported`, or
`unrun`. The harness never builds Pumpkin and never accepts the Minecraft EULA.

Pumpkin:

```sh
uv run --no-project python tools/ironore/qualify.py \
  --backend pumpkin \
  --executable /absolute/path/to/pumpkin \
  --run-dir /absolute/path/to/new-run-directory \
  --timeout 240
```

Official vanilla 26.3:

```sh
uv run --no-project python tools/ironore/qualify.py \
  --backend vanilla \
  --vanilla-jar /absolute/path/to/minecraft-server-26.3.jar \
  --eula-source /absolute/path/to/already-accepted/eula.txt \
  --run-dir /absolute/path/to/new-run-directory \
  --timeout 240
```

`--timeout` is an overall wall-clock bound covering both starts and all checks.
The vanilla run copies the already-accepted EULA supplied with
`--eula-source`; it refuses to run if that source does not contain `eula=true`.
Server output is retained in `harness-server.log`. Exit status is 0 when every
check passes, 1 when any check fails, and 2 when the result is only partial
because a check is unsupported or unrun.

Static validation:

```sh
uvx mypy --strict --explicit-package-bases \
  tools/ironore/qualify.py tools/ironore/test_qualify.py
python -m unittest tools.ironore.test_qualify
```


## Teleport acknowledgements

The additional protocol suite exercises accepted positions and rotations,
superseded acknowledgements before and after completion, invalid coordinates
and rotations, and duplicate confirmations. It writes every observation and
the executable hash to `teleport-qualification.json`.

```sh
uv run --no-project python -m tools.ironore.qualify_teleports \
  --executable /absolute/path/to/pumpkin \
  --run-dir /absolute/path/to/new-teleport-run \
  --timeout 240
```
