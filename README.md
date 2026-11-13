# ok_q3_converter

Converts your own Quake III Arena game files (the id Tech 3 `.pk3` stack: `baseq3`, plus `missionpack` if present)
into modern files: the maps, player models, weapons and other models as glTF, images as PNG, sounds as Ogg Vorbis,
videos as WebM and the shader scripts as JSON, with an HTML5 viewer that draws the maps with their shader effects. As
a library it also converts the maps into runtime data, step by step, in native and WebAssembly programs.

> **Unofficial.** ok_q3_converter is not affiliated with, endorsed or sponsored by id Software or ZeniMax Media.
> Quake is a registered trademark of ZeniMax Media Inc.; id Software is a ZeniMax Media company. This repository
> contains no game data: you need your own legally obtained copy of the game, whose files the converter reads on your
> computer. Do not ask for, or share, game files or converted files.

## Quick start

You need [Rust](https://rustup.rs) and your Quake III Arena installation (the folder holding `baseq3/`); the videos
also need [ffmpeg](https://ffmpeg.org) on your PATH.

```sh
cargo build --release
target/release/okconv-q3 convert --game "path/to/Quake 3" --out output
target/release/okconv-q3 serve output          # the viewer at http://127.0.0.1:8077
```

## Commands

| command | what |
|---|---|
| `okconv-q3 convert [--game DIR] [--out DIR] [--only GROUPS] [--map-filter TEXT]` | converts everything; `--only maps,players,weapons,models,textures,sounds,videos,misc,viewer` runs selected groups (the manifest is merged), `--map-filter q3dm17` only the maps whose name contains the text |
| `okconv-q3 serve [DIR] [--port N]` | serves an output folder and opens the viewer |

The paks are read from their files as needed rather than loaded whole (about 330 MB peak memory instead of 1.1 GB for
a full `baseq3` run).

## Output (`output/<mod>/…`)

| from the paks | output |
|---|---|
| `maps/*.bsp` | `maps/<name>.glb`: Bézier patches tessellated, lightmap atlas on `TEXCOORD_1` (also bound as occlusion), vertex colours, brush entities as nodes, items placed at spawn points with an `item_spin` clip, entity key/values in node `extras`; plus `<name>.entities.json` and `<name>_lightmap.png` |
| player MD3s + `.skin` + `animation.cfg` | `players/<model>_<skin>.glb`: head, upper and lower body joined through animated tag nodes; 25 (31 for Team Arena) named clips (`LEGS_RUN`, `TORSO_ATTACK`, `BOTH_DEATH1` …) as morph-target animation; a `tag_weapon` node to attach weapons |
| weapon MD3s | `weapons/<name>.glb`: model + barrel (`barrel_spin` clip) + muzzle flash |
| every `.md3` | the same path as `.glb` (multi-frame models get a `frames` clip) |
| `.tga` / `.jpg` | `.png` (UI art grouped separately in the manifest) |
| `.wav` | `.ogg` (Vorbis, encoded in Rust) + the original `.wav` |
| `video/*.roq` | `.webm` (VP9 / Opus, needs `ffmpeg`) |
| `scripts/*.shader` | `shaders.json` (every shader's parsed stages) |
| everything else | `misc/<path>`, raw copies |

Coordinates: glTF Y-up, 1 Quake unit = 0.0254 m. Materials carry `extras.q3shader`, so other tools can map the
shader effects; textures are referenced by relative URI, so keep the folder structure.

## Viewer

Tabs: Maps (fly camera), Players (legs / torso clips, weapon attach, voice sounds), Weapons, Models, Shaders/VFX,
Textures, UI/HUD, Sounds, Music, Video, Files. `viewer/q3shader.js` draws the shaders: multi-pass stages, blendFunc,
depthFunc / depthWrite, alphaFunc, tcGen (environment / lightmap / vector), tcMod (scroll / scale / rotate / stretch /
transform / turb), rgbGen / alphaGen (wave / const / vertex / lightingDiffuse), animMap, videoMap, deformVertexes
wave / bulge, sky box + cloud layers, fog volumes. "Shader FX" switches back to the plain glTF materials. The viewer's
files are embedded in the binary, so rebuild after editing `viewer/`.

## Library and runtime data

Features: `cli` (default: the command line tool, the viewer and its server, sounds through Vorbis and videos through
ffmpeg) and `runtime`. Without `cli` the library has no file system access and runs no external programs, and it
builds for `wasm32-unknown-unknown`:

```sh
cargo build --lib --no-default-features --features runtime --target wasm32-unknown-unknown
```

`runtime::Converter` converts the maps of a `baseq3` pak stack into runtime data, in the CLI's layout:
`shaders.json`, every map's `.glb`, lightmap and entities, every image the maps use (plus `env/*` and `levelshots/*`)
as PNG, and `manifest.json` (`game`, `format`, `paks`, `maps`, `skipped`, `files` with size and CRC-32). The paks come
in as readers (`runtime::Paks::open`; only their zip directories are read until a file is needed), the files go out
through a `runtime::Sink`; `Converter::step` runs one step at a time (a caller can draw progress between them),
`Converter::run` all of them. Every file is byte-identical to the CLI's at the same path.

## Development

```sh
cargo fmt --check && cargo clippy --release --all-targets --all-features -- -D warnings
cargo test --features runtime                                       # synthetic paks
Q3_BASEQ3="path/to/Quake 3/baseq3" cargo test --release --features runtime -- --ignored --nocapture
```

The last one converts the real paks and compares them with a full CLI output (`Q3_CLI_OUTPUT`, default `output/`).

## License

MIT ([LICENSE](LICENSE)). Third-party code and its licenses: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
