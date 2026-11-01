# MailCraft app icon

An **envelope that is also a calendar page**, with two ring tabs at its top, in front of two
tilted sheets, on a rounded tile, in blue: mail, calendar and the rest of the program in one mark.

The design is Martin Ball's. The SVG was drawn by hand as vectors for MailCraft (not traced, and
nothing in it comes from any other product's icon). It is original work of the MailCraft
contributors, under the project's licence (MIT OR Apache-2.0).

## Files

| Path | What |
|---|---|
| `source/mailcraft.svg` | The source: everything else here is written from it |
| `alternates/calmail-{teal,violet,sunset,green,graphite}.svg` | The same drawing in other colour schemes, kept for the future and not used yet |
| `alternates/gen.py` | Writes every scheme, blue included, from one template and a palette per scheme |
| `hicolor/` | Linux: PNGs from 16 to 512 px, and the SVG as the scalable icon |
| `mailcraft.ico` | Windows: 16 to 256 px |
| `mailcraft.icns`, `mailcraft-macos-512.png` | macOS, on Apple's 824/1024 grid |
| `mailcraft-1024.png` | The icon at 1024 px |

## Geometry

The tile is fitted to the canvas, whatever margin the SVG keeps round it (the bounding box of
what it draws, made square about its centre); the scalable icon's `viewBox` is set to the same
square. The macOS files keep Apple's margin (an 824 px body on a 1024 px canvas). The 16, 24, 32
and 48 px files show the drawing a little closer inside the same rounded tile (1.14, 1.08, 1.08
and 1.04 times), so the page and its envelope still read.

## Changing it

Edit `source/mailcraft.svg` (or the template in `alternates/gen.py`, then copy its blue output
there), then:

```sh
cargo xtask icon
```

It renders the SVG with `resvg`, a pure-Rust renderer that only xtask depends on, and writes every
file above. Commit what changed; each file has its row in `ATTRIBUTION.md`.
