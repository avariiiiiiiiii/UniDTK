# ASCII Graphics Research and Concept for UniDTK (`docs/unidtkascii.md`)

> **Purpose of this document:** Records the requirements, concepts, experiments, technical constraints, and drafts for the ASCII/Unicode identity and the graphic logo of the `UniDTK` project.

In this document you will learn how the UniDTK emblem is made. Nobody draws it by hand. A script computes a vector model of the emblem, samples it onto a dot grid, converts the grid into Braille characters, and writes the result into a Rust source file that the `udtk` command prints as its splash screen. You will also learn how each step is checked: by the generator's own audits, and by a separate tool that reads back the bytes the real binary printed. Section 4 walks through the pipeline in order. The sections before it explain the design and the problems the pipeline solves.

---

## 1. Identity Concept (The Visual Concept)

The main visual idea of the `UniDTK` logo:

* **Central core: the planet Saturn:**
  * A symbol of time (Chronos), solidity, and the stability of the operating environment's core and of the microkernel architecture.
  * A spherical body with latitudinal cloud bands, a polar vortex, and a tilted equatorial plane (~26.7°).
* **Framing ring: the Hexagonal Ouroboros:**
  * A mythical serpent/dragon holding its own tail in its jaws.
  * A symbol of a continuous life cycle, a closed self-contained ecosystem, self-repair, and protection.
  * **Hexagonal shape (Hexagon):** stands for modularity (the 6 main modules of the system / honeycomb structure), engineering rigor, and a cybernetic shield.
* **Text block:**
  * A block-style monospace lettering of the name `UNIDTK` (a corrected version without kerning defects).

---

## 2. Where ASCII Graphics Are Used in UniDTK

1. **Header of `README.md`:**
   * The brand header art at the top of the repository docs, **shipped** (the compact `88`-cell variant, see `5.5`).
   * A tree of the file structure (`core/`, `modules/`, `opt/`, `toolchain/`).
2. **Architecture diagrams (`docs/`):**
   * Diagrams of the system layers (core, event bus, Tauri/CLI/AI bridges, the `unidtk-api` contract).
   * Diagrams of the split between `Widgets` and `Surfaces` objects (`unicoremodmotion.md`).
   * Diagrams of the life cycle and of the 10-step development cycle in `AGENTS.md`.
3. **The `udtk` CLI tool (`opt/cli`):**
   * A splash screen when run **with no arguments** (`udtk`), **shipped**: the emblem on the left, the block lettering `UNIDTK` on the right, and below it a map of the commands in plain text, all in color (see `6`). `udtk help` / `-h` / `--help` deliberately stay plain, uncolored text, so their output remains usable in scripts.
   * A header for the system diagnostics `udtk doctor` and the `udtk status` summary, **not shipped**.

> [!IMPORTANT]
> **Architecture rule:** `UniDTK` is written in plain Rust/Tauri/JS and **must not depend on Python or third-party graphics libraries under any circumstances**. All final ASCII graphics are stored only as static UTF-8 text in the code and the docs. The Python scripts in section 4 are a development tool outside the repository. The product never runs them.

---

## 3. Why Drawing Is Hard: Pitfalls and Lessons Learned

A series of practical experiments revealed the main difficulties in generating complex figurative ASCII graphics. Each item is a constraint the pipeline in section 4 is built around.

1. **Distorted proportions (Aspect Ratio):**
   * Monospace characters in terminals and text editors have an aspect ratio of about `1:2`. A character cell is nearly twice as tall as it is wide.
   * Any mathematical circle drawn without a compression factor (`~2.05`) turns visually into a squashed, vertical oval.
2. **The problem with procedural pseudo-graphics:**
   * Filling geometric areas with procedural noise (`░▒▓█`) reads to the human eye as digital garbage or "corrupted memory", not as a snake's body or scales.
   * To recognize a living creature, you need continuous outline strokes, a clearly drawn jaw with teeth, an eye, and a tapering tail. Gradient blotches do not work.
3. **Limits of text LLMs:**
   * A text model does not "see" the 2D canvas while it generates character by character. As a result, attempts to draw a complex layered 3D scene by hand (sphere + rings + the serpent's coils + jaw) produce broken lines and artifacts.
4. **Solution to the detail problem:**
   * A specialized local agent skill, `ascii-art` (`C:\Users\admin\Documents\skills\ascii-art`), was connected.
   * The skill supports a subpixel Braille grid (8 dots per character), matrix dithering (Floyd-Steinberg), and vector outlines (`edge`). It runs in an isolated `.venv`, so it does not affect the project.

### 3.1 What Blocked the Drawing (Pitfalls of the Second Pass)

5. **Blind character generation does not scale. You need feedback from the picture.**
   * The model does not "see" the text canvas, but it **does see images**. The working loop: vector drawing → dot grid → conversion to Braille → **reverse rendering of the resulting text to PNG and a visual audit**. Without this step, every anatomy fix is guesswork.
   * The reverse rendering must copy the physics of the terminal: one Braille dot = one square subpixel in a 1:2 cell (2 × 4 dots per character).
6. **The ribbon polygon crosses itself.** Filling the snake's body with two offset outlines degenerates into a narrow "blade" at the rounded corners.
   * Fix: the body is drawn with a **variable-radius brush** (stamped circles along the centerline). It does not cross itself, the corners round off naturally, and the tail tapers smoothly.
7. **The path needs dense sampling.** Straight hexagon sides without intermediate points break the arc-length anchoring: the "head" snaps to a vertex about 30 cells off target.
8. **Compositing order matters more than geometry.**
   * `Image.paste(layer)` without a mask overwrites the whole canvas. Both the body and the head disappear.
   * Saturn's ring needs three passes: the full elliptical system → "punching out" the planet's disk (this hides the far half of the ring) → redrawing the near half over the disk.
   * The sphere's silhouette is drawn **after** the masked insertion of details. Otherwise the mask erases it.
9. **We fully control the binarization threshold.** The Braille skill uses a threshold equal to the average brightness of the frame. So a deliberate binarization of the source (in our case, `0.40` coverage) fully determines the final dots, and antialiasing only produces soft edges.
10. **Half-tones on a sphere require your own dithering.** The planet's terminator (shadow) is applied to the dot grid with a `4×4` Bayer matrix, not a gradient. Braille reproduces a prepared 1-bit raster exactly, but any "gray" gradient collapses into one hard edge at the average value.
11. **The anatomy of the jaw dictates the geometry of the whole pattern.** The tail must enter the open jaw. The position of the tail tip and the lengths of the jaws depend on each other. If the tail tip ends up behind the tip of the lower jaw, the bite does not read at all.
12. **The snake's body is segmented skin, not a solid fill.** After the body is filled, transverse scale rows are cut into it: arc-shaped "chevrons" with a pitch of ≈ `6.8` dots and a depth of ≈ `2` dots. The pitch, depth, and thickness have a slight sinusoidal irregularity. The outer contour is kept (the scales stop about `1` dot short of the edge), so the body reads as alive, not as a smooth hoop. This is not procedural noise (§3, item 2). It is a regular, deterministic rhythm: noise breaks the silhouette, while the rhythm emphasizes it.
13. **The head holds details only at its own scale.** Working set: an almond-shaped eye with a slit pupil (the pupil is a narrow vertical lens), three pairs of fangs per jaw (the front pair is the longest), a nostril, and dorsal scale rows on the skull that continue the body's pattern. A large round eye turned the head into "one big eye", so it was made smaller. The head's "neck" is sunk into the body so there is no step at the joint.

---

## 4. The Pipeline, Step by Step

The emblem goes through six stages. Each stage reads the output of the one before it. The stages live in the skill folder `C:\Users\admin\Documents\skills\ascii-art\scripts`, next to the source painting, and outside the repository (section 9 explains why).

```
vector model ──► dot grid ──► Braille text ──► trimmed grid + shade ──► splash_art.rs ──► printed bytes
 (unidtk_emblem.py)  (220×188)   (skill)        (make_splash.py)          (Rust source)     (audit_capture.py)
```

### 4.1 Step 1: The Vector Model (`unidtk_emblem.py`)

The shape is not traced from a picture. It is computed from numbers. The generator keeps one configuration dictionary, and each key is a visible property of the emblem:

```python
# Filename: C:\Users\admin\Documents\skills\ascii-art\scripts\unidtk_emblem.py (default_cfg, abridged)
dots_w=220, dots_h=188,
R=84.0, rc=20.0, cx=110.0, cy=94.5,
body_half=6.2, swell=0.7, tail_len=26.0, tail_cut=40.0, tip_reveal=22.0,
head_scale=1.2,
planet_dy=11.0, sphere_r=36.5, tilt_deg=26.7,
scale_pitch=6.8, scale_w=1.15, scale_depth=2.05, scale_inset=1.0,
scale_start=8.0, scale_min_half=2.6,
head_scale_rows=[1.5, 6.5, 11.5], head_scale_bow=2.0, head_scale_w=1.05,
shade=0.85, term=0.25,
```

Read the keys as the design:

- `R` and `rc` are the hexagon's circumradius and corner rounding (`RC = 20.0  # corner rounding: a hexagon, not a circle`). The hexagon is the Ouroboros frame.
- `body_half`, `swell` and `tail_len` set the thickness of the serpent along its path. The body is stamped as circles of varying radius (pitfall 6).
- `sphere_r`, `tilt_deg` and `planet_dy` place Saturn. `tilt_deg` is the axis tilt of 26.7°.
- `scale_pitch`, `scale_depth` and `scale_w` are the scale rhythm of pitfall 12.
- `head_scale_rows` and `head_scale_bow` place the skull scale rows of pitfall 13.
- `shade` and `term` belong to the planet's shading (step 2).

The generator draws every part into the dot grid. Its geometry is deterministic, so the same configuration always gives the same picture.

### 4.2 Step 2: The Dot Grid and the Planet's Terminator

The vector image is binarized into a dot grid of `220 × 188` dots. Each dot is either ink or empty. The default threshold is `--thr 0.40`, and the flag is parsed from the command line:

```python
# Filename: unidtk_emblem.py (argument parsing, abridged)
ap.add_argument("--dots-w", type=int, default=220)
ap.add_argument("--dots-h", type=int, default=188)
ap.add_argument("--thr", type=float, default=0.40)
```

The planet's shadow is not a gradient. It is an ordered dither: the code turns the shading strength into a density, scales it by the limb fade and by the `shade` setting, and compares it with a fixed 4×4 threshold matrix:

```python
# Filename: unidtk_emblem.py (terminator, abridged)
dens = np.clip((cfg["term"] - s) / (1.0 + cfg["term"]), 0.0, 1.0) ** 1.3
edge = np.clip((1.02 - dist) / 0.22, 0.0, 1.0)     # fade out at the limb
dens = dens * edge * cfg["shade"]
bayer = (np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]) + 0.5) / 16.0
tmap = np.tile(bayer, (h // 4 + 1, w // 4 + 1))[:h, :w]
return np.where(dens > tmap, 0, dots)
```

Where the density is above the matrix value, the dot is cleared to `0`. Elsewhere the dot keeps the value it had in the vector render. So the dark side of the planet is stippled with cleared dots, and the pattern looks like a halftone at the dot scale. Braille can show such a pattern exactly, because every dot is either on or off (pitfall 10).

### 4.3 Step 3: From Dots to Braille Characters

One Braille character holds a 2 × 4 block of dots. Unicode gives each of the 256 possible patterns its own code point, starting at `U+2800`. The character for a block is `U+2800` plus the 8-bit mask of the dots that are on. The reverse is `code = ord(ch) - 0x2800`, which the generator uses when it reads the characters back:

```python
# Filename: make_splash.py (cell conversion, abridged)
code = (ord(ch) - 0x2800) if ch != " " else 0
cells.append(" " if code == 0 else chr(0x2800 + code))
```

The empty pattern is `U+2800`. The generator writes it as a plain space, so the repository contains no invisible characters. The skill does the conversion itself at the requested character width.

### 4.4 Step 4: Trimming and the Photo Texture

The shape is then trimmed. Empty rows at the top and bottom are removed, and the number of removed rows is kept as `top_off`. The geometry needs that number to map the splash back onto the original dot grid.

The serpent's brightness comes from the source painting (section 8). For each character cell, the generator takes the mean of the lit dots under it, and then turns it into a byte. The byte is `128` for neutral and is quantized in steps of `8`:

```python
# Filename: make_splash.py (cell shade, abridged)
q = round((mean - NEUTRAL) / SHADE_STEP) * SHADE_STEP
```

The constants are `NEUTRAL = 128`, `SHADE_STEP = 8`, and `SHADE_CLAMP = 48`. So the byte stays between `80` and `176`. Saturn and the rings are never shaded, because the runtime only applies the shade to the serpent's body (section 6).

### 4.5 Step 5: Scale Factors and Geometry

The runtime needs to know how a character cell maps back to the dot grid, so `make_splash.py` writes two scale factors for each tier:

```python
# Filename: make_splash.py (scale computation, abridged)
scale_x = DOTS / (2.0 * cols)
scale_y = DOTS / (4.0 * rows_untrimmed)
```

`DOTS` is `200` (the square grid the emblem is drawn on). `cols` is the character width of the tier. `scale_y` uses the number of rows **before** trimming, which is why `top_off` is kept. The two factors are equal when the grid is square, and the code expects that (`scale_x == scale_y` in the file header).

### 4.6 Step 6: The Generated Rust File (`opt/cli/splash_art.rs`)

The generator writes the grids, the shade bytes and the geometry into one file. The file starts with a warning and the provenance of the picture:

```rust
// Filename: opt/cli/splash_art.rs (header, abridged)
//! Generated art for the `udtk` splash screen. **Do not edit by hand.**
//!
//! Regenerate with `make_splash.py` (docs/unidtkascii.md §5):
//!
//!     make_splash.py --repo <UniDTK repo root>
```

Each tier has three parts: the rows of characters, a `Geo` value with the column count, `top_off` and the scales, and a flat array of shade bytes with one byte per cell:

```rust
// Filename: opt/cli/splash_art.rs (types and planet, abridged)
pub struct Geo {
    /// Character columns of the block (== every line's length).
    pub cols: usize,
    /// Blank cell rows trimmed from the top.
    pub top_off: usize,
    /// Generator dot units per braille dot, horizontally / vertically.
    pub scale_x: f32,
    pub scale_y: f32,
    /// Per-cell shade, row after row (`cols` bytes per row, 128 == neutral).
    pub shade: &'static [u8],
}

/// Saturn in generator dot coordinates: centre x, centre y, radius.
pub const SPHERE: (f32, f32, f32) = (100.0, 100.0, 30.0);
pub const RINGS: [(f32, f32); 3] = [(52.0, 13.8), (45.0, 12.0), (38.0, 10.1)];
pub const TILT_DEG: f32 = 26.7;
```

The file holds three emblem tiers, each with its own `Geo` and shade array:

| Tier | Constant | Character columns | Rows | Shade bytes |
|------|----------|-------------------|------|-------------|
| XL | `EMBLEM_XL` | 48 | 21 | 1008 (`48 × 21`) |
| Wide | `EMBLEM_WIDE` | 36 | 16 | 576 (`36 × 16`) |
| Compact | `EMBLEM_SMALL` | 28 | 13 | 364 (`28 × 13`) |

The file also holds the wordmark rows (`WORDMARK`, six rows) and the planet and ring constants. The wordmark is a separate six-row block. It is not the 4.1 block copied verbatim (section 10).

### 4.7 Step 7: The Built-in Audits of the Generator

`make_splash.py` does not trust its own output. Before it writes the file, it runs several checks, and it stops with an error if one fails:

- **Shape of the data.** The width of each line equals `geo.cols`, and it equals the length of the shade row. So the runtime can index the shade array with the same column number it uses for the text.
- **Alphabet.** Each line may contain only a space or a Braille character in `U+2801…U+28FF`. No ESC byte and no invisible character is allowed.
- **Shade range.** Every byte is neutral or a quantized value inside `80…176`.
- **Zone check.** For each cell centre, the generator decides which zone it lies in (planet, rings or body). It compares that decision with the ground truth of each Braille dot. The script prints the agreement, in the form `geometry: %d/%d cells agree with per-dot truth (%.1f%%)`. The old text recorded `97…99%` agreement with all three zones present. That figure was not re-run for this rewrite.
- **Determinism.** The format is deterministic. A repeated run produces the same file, byte for byte. `--check` returns a non-zero exit code if `splash_art.rs` was edited by hand or no longer matches the generator.

---

## 5. Recorded Variants and Prototypes

> **Status:** variants `5.2` and `5.3` are unimplemented sketches from earlier passes (broken lines, lost proportions). Variant `5.4` is the **final verified art**, and it is the reference for the identity. Variant `5.5` is the compact copy shipped in the README.

The numbering below keeps the original section numbers in the file history (`4.1` to `4.5` in the old text). The art itself is unchanged.

### 5.1 Base Lettering Block `UNIDTK` (verified, clean)

```text
██╗   ██╗███╗   ██╗██╗██████╗ ████████╗██╗  ██╗
██║   ██║████╗  ██║██║██╔══██╗╚══██╔══╝██║ ██╔╝
██║   ██║██╔██╗ ██║██║██║  ██║   ██║   █████╔╝ 
██║   ██║██║╚██╗██║██║██║  ██║   ██║   ██╔═██╗ 
╚██████╔╝██║ ╚████║██║██████╔╝   ██║   ██║  ██╗
 ╚═════╝ ╚═╝  ╚═══╝╚═╝╚═════╝    ╚═╝   ╚═╝  ╚═╝
```

---

### 5.2 Prototype: Compact Cyber Symbol of the Ouroboros

```text
                                  .────════════════════════────.
                             _.-'´              ::              `'─._
                        _.-'´          .---''''''''''''---.          `'─._
                   _.-'´             .'     .::::::::.     '.             `'─._
              _.-'´      ▲          /     .::::::::::::.     \          ▲      `'─._
         _.-'´          / \        |     .::::::::::::::::.   |        / \          `'─._
        <══════════════╡   ╞═══════|     ::::::::::::::::::   |═══════╡   ╞══════════════>
         `'─._          \ /         \     '::::::::::::::::/  /        \ /          _.-'´
              `'─._      ▼           '.     '::::::::::'     .'         ▼      _.-'´
                   `'─._               '---............---'               _.-'´
                        `'─._       ░░▒▒▓▓████████████▓▓▒▒░░         _.-'´
                             `'─._    ◄◄◄===(( ◉=8=◉ ))===►►►   _.-'´
                                  '────════════════════════────'
```

---

### 5.3 Prototype: Detailed Figurative Sketch

```text
                                          _...--──══════════════════════──--..._
                                   _..-─'´    ▲   ▲   ▲   ▲   ▲   ▲   ▲   ▲   `'─-.._
                            _..-─'´  __...--──══════════════════════════════──--...__  `'─-.._
                     _..-─'´   _.-'´´   \  /  \  /  \  /  \  /  \  /  \  /  \  /   ``'-._   `'─-.._
              _..-─'´   _.-'´´           \/\/\/\/\/\/\/\/\/\/\/\/\/\/\/\/\/\/\/\/         ``'-._   `'─-.._
       _..-─'´   _.-'´´                  ≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡≡               ``'-._   `'─-.._
    .-'´   _.-'´´                                                                               ``'-._  `'-.
  .'   _.-'                                  _..---'''''''''''''''''''---.._                                 '-._  '.
 /   .´       ▲                           .-'´      .::::::::::::::::.      `'-.                          ▲      `.   \
|   /        /  ▲  \                    /       .::::::::::::::::::::::::::::.       \                   /  ▲  \       \   |
|  |        /  / \  \                  |       .::::::::::::::::::::::::::::::.       |                 /  / \  \       |  |
|  |       \  \   /  /                 |      ::::::::::::::::::::::::::::::::::      |                \  \   /  /      |  |
|  |        \  \ /  /                  \      '::::::::::::::::::::::::::::::::/      /                 \  \ /  /       |  |
|  |         \_/                        '.       '::::::::::::::::::::::'       .'                       \_/          |  |
 \  \   /\/\/\/\/\                   <==========================================>                     /\/\/\/\/\   /  /
  \  \  ══════════                  /   .------------------------------------.   \                    ══════════  /  /
   \  \                            /   /          .::::::::::::::::.          \   \                              /  /
    \  \                          |   |       ::::::::::::::::::::::::::       |   |                            /  /
     \  \                          \   \      '::::::::::::::::::::::::/      /   /                            /  /
      \  \                        <==========================================>                          /  /
       \  \                  _...---.._                       _...----------------..._               /  /
        \  \              .-'   _      `'-.                .-'  _  _  _  _  _  _  _   `'-.          /  /
         \  \           .'     (◈)         `'--.._       .-'  _  _  _  _  _  _  _  _    \        /  /
          \  \         /     .---'                `'-..-/    / \/ \/ \/ \/ \/ \/ \/ \    \      /  /
           \  \       |     /   \=====VVVVVVVVVV=====\      |  |  |  |  |  |  |  |  |     |    /  /
            \  \      |    |     |  ◄═══════════════════════|  |  |  |  |  |  |  |  |     |   /  /
             \  \      \    \   /=====^^^^^^^^^^=====/       \  \  \  \  \  \  \  \  \    /   /  /
              \  \      '.   '---'                            '--'--'--'--'--'--'--'--'  /   /  /
               \  \       '-._                    _...-------------------------------...-'   /  /
                \  '─-.._     `'--............--'´                                     _...-─'´  /
                 `'─-..___`'─-..________________________________________________...-─'´___..-─'´
```

---

### 5.4 Final `UNIDTK` Emblem (reference, 110 cells)

**Composition:** a regular hexagonal Ouroboros with a **scaled body** (rounded edges, variable thickness, transverse scale rows along the whole length, a tapering tail that enters the open jaw). It wraps around Saturn (axis tilt `26.7°`, three elliptical ring ellipses, latitudinal bands, a hexagonal polar vortex, and a dithered terminator for volume). Head: an almond-shaped eye with a slit pupil, three pairs of fangs per jaw, a nostril, and dorsal scale rows on the skull.

**Technical details:** a Braille grid of `220 × 188` dots. The output is `99` cells × `42` lines after the empty edges are cropped.

```text
                                    ⣀⣀⣀⣤⣤⣤⣤⣶⣶⣶⣶⣶⢲⣶⣶⢤⣤⣀
                                ⣴⡾⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣆⢿⣿⣎⢿⣿⡹⣶⣶⣶⣤⣤⣄⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⡀
                                ⠈⠁⠈⠉⠉⠛⠛⠛⠛⠛⠛⠻⠿⠿⠿⣿⡟⣼⣿⡟⣼⣿⡷⣹⣿⣿⣿⣿⣿⣿⣯⢻⣿⣯⢹⣿⣿⡌⣿⣿⣿⡏⣷⣶⣤⡀
                                 ⢀⣠⣄         ⢀⣤⣾⣴⣿⣿⣴⣿⣿⣵⣿⣿⣿⣿⣿⣿⣿⣿⢎⣿⣿⢂⣿⣿⡿⢸⣿⣿⡇⣻⣿⡟⣽⣷⣄
                               ⣠⣶⣿⡿⠛  ⣀⣤⣤⣴⣶⣾⣿⣿⣿⠋⠉⣴⠈⠙⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣏⣼⣿⣟⣸⣿⣿⣣⣿⣿⢟⣴⣿⠟⣱⣿⣿⡿⢳⡄
                             ⣠⣾⣿⣿⠋   ⠛⠿⣿⣿⣿⣿⣿⣿⣿⣿⣦⣀⣛⣀⣤⣿⣿⣿⣿⣿⠿⠟⠛⠁          ⠉⠉⠻⢵⣾⣿⡿⠟⣱⣿⣿⣄
                           ⣠⣾⣿⣿⣿⡏       ⠈⠉⠙⠛⠛⠛⠻⠿⠿⠿⠟⠛⠛⠋⠉                    ⠹⣶⣾⣿⣿⣿⡿⣹⣆
                         ⢀⣼⣿⣿⣿⣿⡿                                            ⠘⣟⣛⣛⣫⣾⣿⡿⣦
                        ⣰⠿⣿⣿⣿⣿⣿⠃                                             ⠘⢿⡿⠿⠿⢋⣼⣿⣧⡀
                       ⣰⣿⣿⣿⣿⡿⣿⠏                                               ⠈⢷⣾⣿⣿⣿⢟⣵⣷⡀
                      ⣼⣭⣍⣛⢿⣿⣿⠏                                                 ⠈⢻⣯⣭⣶⣿⣿⣿⢷⡄
                    ⢀⡼⠿⢿⣿⣿⣦⣹⠃                                                    ⢻⣿⣿⠿⠟⣡⣾⣿⣄
                   ⢀⣾⣿⣿⣿⣎⢿⡿⠃                                                      ⠱⣶⣾⣿⣿⡿⢫⣾⣆
                  ⢀⣾⣭⣭⡙⢿⣿⡿⠁                                                        ⠹⣯⣭⣭⣴⣿⣿⢟⣦
                 ⢠⠿⢿⣿⣿⣿⣦⡟                                                           ⠘⣿⠿⠿⢛⣡⣾⣿⣧⡀
                ⣠⣿⣿⣿⣦⡙⣿⠏     ⣀⣀⣀⣀               ⢀⣠⣤⣴⣶⣶⣶⣶⣶⣶⣦⣤⣄⣀                       ⠘⢿⣿⣿⣿⣿⢟⣵⣷⡀
               ⣰⣯⣭⣍⠻⣿⣿⠋  ⣠⠖⠋⠉⠉⢉⣉⣉⣉⣉⣙⣓⠒⠦⠤⣄⣀⡀ ⢀⣠⣶⠿⠛⠋⠉       ⢠⠽⠛⠛⠿⣶⣄⡀                    ⠈⢿⣩⣭⣴⣿⣿⡟⣳⡄
              ⣴⣛⣛⠿⣿⣷⣽⠃   ⡇  ⣠⠚⠉⠁ ⢀⣀⣈⣉⣉⣙⣓⠒⠮⣭⣷⣿⣋⣀           ⠸⡆⠁⠰⠇ ⢹⠻⣶⣄                    ⢻⡿⠿⢟⣫⣾⣿⣿⣄
             ⣼⣿⣿⣿⣷⣝⡿⠁    ⢻⡀ ⢿  ⢠⠞⠉     ⠉⢉⣿⠟⠉⠉⠉⠉⠉⠛⠳⠶⣤⣄⡀     ⠑⠣⣄⣁⡤⠟⠅⠉⠿⣷⡀                   ⠻⣿⣿⣿⣿⡟⣡⣿⣆
            ⣸⣩⣭⣙⠿⣿⡟⠁      ⠱⣄⠘⢆ ⠸⡄      ⣰⡿⢷⡀         ⠈⠉⠛⠶⢤⣀⡁ ⠁⠄⠁⠅⠅⠅⠅⠅⠉⢿⣆                   ⠹⣭⣭⣴⣾⣿⣿⣿⡄
           ⢀⣿⣿⣿⣿⣷⡾         ⠈⢢⡈⠳⡀⠹⣄    ⣰⡿⠁ ⠙⢦⡀            ⠈⠙⠷⢧⣄⠁⠅⠅⠅⠕⢅⠕⠅⢻⣆                   ⢹⣟⣻⣿⣛⣭⣾⣧
           ⢸⣟⣭⣶⣭⣙⡇           ⠙⠢⣘⠦⡈⠳⣄ ⢰⣿⠁    ⠙⢦⣄         ⠁ ⠁⠄⠁⠍⠝⠷⣕⣅⠕⢅⠕⢅⠍⢿⡇                   ⡿⠿⣿⣿⣿⡿⢿
           ⠘⣿⡿⢛⣛⣛⣇             ⠈⠳⣌⠳⣌⠳⣾⡇       ⠈⠳⢦⣀      ⠁⠄⠁⠄⠁⠅⠅⠅⠕⢝⠷⣕⢕⢕⠕⠽⣿                  ⢠⣿⣷⣶⣤⣶⣶⡿
            ⢻⣴⣿⣿⣿⣿⣆               ⠙⢮⣑⣿⣤⣤⣤⣀⣀⡀     ⠉⠓⢦⣄⡀⠁ ⠁⠄⠁⠅⠅⠅⠕⢅⠕⢕⠕⢝⢟⢷⣕⠅⣿⡄                ⢠⣿⣮⡻⣿⣿⣿⣿⠇
            ⠈⢿⣿⢋⣵⣶⣾⣆                ⢸⣿    ⠉⠉⠛⠳⠶⣤⣀⡀  ⠈⠙⠳⢤⣁⡄⠁⠅⠅⠅⠕⢅⠕⢕⢕⢕⢝⢵⢝⢧⣿⣇⡀              ⣠⣟⢿⣿⣿⣶⣶⣶⡞
             ⠈⢳⣿⣿⣿⠿⢟⣧                ⣿⡀         ⠈⠙⠓⠦⣅⡀⠁⠄⠉⠝⠷⠧⣵⣅⡕⢕⠕⢕⢕⢕⢝⢵⢕⢍⣿⠧⣝⠦⣀           ⣰⢿⣿⣦⣙⡛⠿⢿⠏
               ⠻⣟⣵⣿⣿⣿⣷⡀              ⢻⡟⢦⡀           ⠁⠍⠓⠦⣅⡅⠅⠅⠕⢍⠝⢟⢷⢷⢽⣵⣝⣵⣕⣽⡿⠳⣌⠳⢌⠳⣄        ⣰⣿⣷⣝⠿⢿⣿⣿⠋
                ⠹⣿⢟⣴⣶⣶⣷⡀             ⠘⣿⡄⠙⠶⣄⡀      ⠁ ⠁⠄⠁⠅⠍⠝⠷⣥⡕⢕⠕⢕⢕⢕⢝⢵⢝⢕⢕⣿⠃ ⠈⠳⣄⠙⢆⠑⢦⡀    ⣼⣍⠻⣿⣿⣿⣷⣶⠃
                 ⠙⣾⣿⡿⣛⣛⣻⡄             ⠘⣷⡄ ⠈⠙⠶⣄⡀   ⠁⠄⠁⠄⠁⠅⠅⠅⠕⢅⠝⢷⣕⢕⢝⢵⢝⢽⠝⢕⣿⠇    ⠈⢧ ⠳⡄⠑⣄ ⢀⡾⣿⣿⣷⣭⣙⣛⡿⠁
                  ⠘⢟⣼⣿⣿⣿⣿⣄             ⠘⢿⣄    ⠙⠳⢧⣄⡁⠄⠁⠅⠅⠅⠕⢅⠕⢕⠕⢕⢝⢷⣝⢵⢝⢕⣵⡿⠃      ⠈⡇ ⠘⡆⠈⢣⣾⣷⡝⢿⣿⣿⣿⡟⠁
                   ⠈⢿⡿⢋⣴⣶⣾⣆              ⠻⣷⣄    ⠁⠌⠙⠷⢧⣅⣅⠅⠕⢅⠕⢕⢕⢕⢝⢕⠝⢷⣵⣿⠿⠥⣤⣀⣀⣀⣀⣀⡤⠞⠁  ⣿⢠⣾⣿⢿⣿⣶⣶⣶⡟
                    ⠈⢳⣿⣿⢟⣛⣿⣦              ⠈⠛⢷⣦⣀ ⠁⠄⠁⠅⠅⠅⠝⠟⠷⢷⣵⣕⣕⣕⡕⣅⣵⡿⠻⢭⣝⣒⠲⠦⠤⣤⣄⣀⣀⣀⣠⡤⠖⢡⠿⣿⣿⣦⣭⣍⣻⠏
                      ⢻⣵⣿⣿⣿⣿⣧⡀               ⠉⠛⠿⣷⣤⣥⣄⣁⣅⠁⠅⣕⣅⣕⣥⣭⣿⠿⠛⠉    ⠈⠉⠙⠒⠲⠦⠤⠤⣤⣤⣤⣴⣿⣷⣞⡻⠿⢿⣿⠋
                       ⠹⣿⢋⣶⣶⣾⣷⡀                  ⠈⠉⠙⠛⠛⠛⠛⠛⠛⠋⠉⠁                  ⣰⣏⢻⣿⣿⣿⣿⡷⠃
                        ⠙⣾⣿⡟⣛⣛⣳⡄                                              ⣼⢿⣿⣷⣭⣛⣛⡿⠁
                         ⠘⣏⣾⣿⣿⠿⠿⡄                                            ⣼⣿⣦⡻⠿⠿⢿⡟⠁
                          ⠈⢿⢏⣴⣾⣿⣿⣆                                         ⢀⣾⣌⠻⣿⣿⣿⣿⠟
                           ⠈⢿⣿⣿⢟⣭⣽⣷⣄                                     ⣀⣴⡏⢿⣿⣷⣮⣭⣭⠏
                             ⠻⣧⣿⣿⡟⣫⣴⣿⠖⣶⣶⡶⢶⣶⣶⢶⣶⣶⢶⣶⣶⣶⢲⣶⣶⡶⣶⣶⡖⣶⣶⣶⢲⣶⣶⡖⣶⣶⡶⣶⣶⣶⢺⣿⡇⣿⣿⣎⡻⢿⣿⡿⠃
                              ⠈⠻⣿ ⣿⣿⢣⣾⣿⣿⢣⣿⣿⡏⣼⣿⠏⣾⣿⣿⢣⣿⣿⣿⢸⣿⡟⣸⣿⣿⢣⣿⣿⣿⢰⣿⣿⢡⣿⣿⡇⣾⣿⣧⡻⣿⣿⣿⡷⠞⠁
                                ⠈⠙⠻⢿⣇⣿⣿⣿⣆⢿⣿⣷⠹⣿⣧⠻⣿⣿⣆⢻⣿⣿⡜⣿⣷⡘⣿⣿⣧⢻⣿⣿⡌⣿⣿⡎⣿⣿⣷⡙⣿⣿⣿⡶⠟⠋
                                     ⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠙⠛⠛⠛⠛⠛⠛⠋⠉⠉
```

---

### 5.5 Compact Variant for `README.md` (88 cells)

The same vector, compressed in width: `80` cells × `34` lines. The head, jaw, and ring stay readable. The terminator's detail is coarser.

```text
                                  ⣀⣀⣀⣀⣀⣀⣀
                         ⢀⣤⣶⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣯⣿⣿⣿⣿⣶⣤⣤⣄⣀⡀⡀     ⣀
                         ⠈⠛⠘⠛⠛⠛⠛⠛⠛⠻⠿⠿⢿⣿⣿⣿⢿⣿⡷⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣟⣿⣿⣿⣷⣶⣤⡀
                         ⢀⣠⣴⣦    ⢀⣀⣀⣴⣾⣷⣿⢿⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣼⣿⡿⣾⣿⡿⣽⣿⣿⣿⣦⡀
                       ⢀⣴⣿⣿⠟⠉⢠⣴⣿⣿⣿⣿⣿⣿⣯⡀⣿ ⣹⣿⣿⣿⣿⣿⣿⠟⠛⠚⠛⠓⠛⠛⠛⠻⠿⢿⣿⣵⣿⣿⢿⣽⣇
                     ⢀⣴⣿⣿⣿⠃   ⠈⠉⠛⠛⠿⠿⠿⣿⣿⣿⡿⠿⠿⠛⠋⠉⠉            ⠉⢿⣿⣷⣿⣿⣟⣧
                    ⣠⣿⣿⣿⣿⡏                                   ⢻⣿⣿⣿⣿⣿⣷⡀
                  ⢀⣼⣿⣿⣿⣿⣿⠁                                    ⠻⣟⣻⣯⣾⣿⣷⡄
                 ⢀⣾⡿⣿⣿⣿⡿⠁                                      ⠹⣿⣿⣿⣿⣿⣿⣄
                ⢀⣾⣿⣿⣿⣝⡿⠁                                        ⠸⣿⣿⣿⣿⣯⣿⣆
               ⢠⣿⣿⣿⣯⣿⡟                                           ⠈⢿⣾⣿⣿⢿⣿⣦
              ⣠⣿⣿⣷⣝⣿⠟                                             ⠘⢿⣿⣷⣿⡿⣻⣧
             ⣰⣿⣿⣟⢿⣿⠋                   ⢀⣀⣤⣤⣤⣤⣤⣤⣀⡀                   ⢻⣿⣷⣿⣿⣿⣷⡄
            ⣼⣿⣿⡻⣿⡿⠃ ⣠⠾⠛⠛⣛⣛⣛⣛⣻⣶⢶⡤⣄⣀⡀⢀⣠⣾⠿⠟⠛⠉⠉⠉⠉⠉⢩⡽⠿⠿⣷⣄⡀                ⠻⣟⣿⣽⣿⣿⣷⡄
          ⢀⣾⣿⣿⣿⣿⡿⠁  ⣇ ⢰⡟⠉⠁⣠⡤⠬⠭⠭⣿⣛⣿⣿⣿⣯⣤⣀⣀⡀     ⠘⣧⡁⢻ ⣹⣻⣷⣄               ⠸⣿⣿⣿⣿⣿⣿⣆
          ⡼⣿⢿⣿⣿⡟    ⠹⣆⢸⡇ ⣟⠁    ⢀⣼⡟⠁   ⠈⠙⠛⠛⠶⣤⣀⡀⠤⠈⠝⠲⢾⡯⣧⣼⠻⣧⡀              ⠙⣿⣿⣟⣯⣾⣿⡆
         ⣸⣿⣿⣿⣽⡏      ⠙⢧⡙⢦⠙⣆   ⢠⣿⠋⠙⢦⡀        ⠉⠙⠻⣦⣎⡑⢺⣟⣿⣿⣲⡟⢿⡄              ⠘⣿⣿⣿⣿⣿⣿⡀
         ⣿⣿⣿⣿⣿         ⠙⢦⣷⢌⡳⣄ ⣾⡇   ⠙⢦⣄      ⢈⡁⢙⠤⣫⣿⣿⣷⣯⢿⣽⣯⡿⣿⡀              ⢹⡿⣿⣿⣿⢿⡇
         ⢹⡿⣻⣿⣿⣄          ⠈⠳⣽⣮⣻⡿      ⠈⠻⣦⣄  ⡄⢨⡕⢨⣖⣽⣿⣺⣟⣿⣻⣿⣿⣟⣻⡇             ⢀⣾⣿⣿⣶⣿⣿⠃
         ⠈⢿⣿⡿⣿⣿⡆           ⠈⠛⣿⡟⠛⠛⠲⠶⣤⣤⣄⡀ ⠉⠛⠶⣤⣠⢌⠤⣯⣯⣽⣽⣯⣿⣽⣿⣿⣿⣽⣿            ⢀⣾⣿⣿⣿⣿⣿⡏
          ⠘⢿⣾⣿⣿⣿⣦            ⢹⣇     ⠈⠉⠙⠳⠶⣴⣀⠂⡝⢻⢶⣷⣷⣻⢾⡷⣿⣿⣿⣿⣿⣻⣿⣷⣤⡀        ⢠⣿⣿⣝⣿⢿⣿⠟
            ⢻⣿⣿⣿⣿⣧           ⠘⣿⣷⣄       ⡀⢈⠹⣛⣾⣭⣹⣧⣿⣽⣻⣿⣿⣿⣿⣿⣿⣿⠟⢮⡳⣝⣦⣀     ⣠⣿⣿⣿⣿⣿⣿⠋
             ⢻⣿⣿⣿⣿⣧⡄          ⢹⣗⠙⠻⣦⣀    ⡄⢠⠐⡤⣷⣾⢽⡷⣿⣿⣻⣿⣿⣿⣿⣿⣿⡏  ⠙⢦⠹⣌⠳⣄  ⣰⣿⣾⣿⣿⣿⡿⠃
              ⠹⣿⣻⣿⣿⣷⡀          ⠻⣷⡀ ⠉⠛⢶⣴⣀⢎⡱⢸⣖⣿⣽⣹⣿⣯⣿⣿⣿⣿⣿⣿⣽⠟⠁   ⠈⢷⠈⢳⡜⢶⣴⣿⣿⣿⣿⣾⡿⠁
               ⠘⣿⡿⣫⣿⣿⡄          ⠘⢿⣦⡀  ⠘⠝⠻⣶⣼⣷⡿⢾⡵⢾⣷⣿⣿⡿⣿⣿⣿⠯⣤⣀⣀⣀⣠⡴⠟ ⢸⣇⣼⣿⣿⣷⣿⣿⡿⠁
                ⠈⢿⣿⣿⣿⣿⣆           ⠙⠻⣷⣄⣉⠆⡹⢻⣻⣟⣛⣿⣿⣿⣿⣿⣿⣽⠟⠻⠿⢿⣻⣶⢶⠦⠤⠤⠤⠶⢟⣿⢿⣿⣶⣿⣽⡏
                 ⠈⢿⣿⣿⣿⣿⣦            ⠈⠙⠻⠻⠿⣶⣾⣤⣷⣿⣾⠿⠟⠟⠋⠁     ⠉⠉⠛⠛⠛⠛⢿⣿⣿⣿⣿⣿⣿⠏
                  ⠈⠻⣾⣿⢿⣿⣧⡀                                    ⢰⣿⣷⣿⣿⣿⣿⠃
                    ⢹⣿⣿⣿⢿⣷⡀                                  ⣠⣿⣯⣿⣿⣿⡿⠃
                     ⠙⣿⣾⣿⣿⣿⣄                               ⢀⣼⣷⣝⣿⣿⣿⡟⠁
                      ⠘⣿⣿⣿⣿⣿⣳⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤⣶⣿⣯⣿⣿⣿⣿⡟
                       ⠈⠻⣿⡏⣿⣿⣯⣾⣿⢯⣿⣿⣻⣿⢿⣿⣿⢿⣿⣿⣿⣿⣟⣾⣿⢿⣿⣿⣧⣿⣟⣽⣿⣯⣿⣿⣿⣿⣿⣿⡿⠃
                         ⠈⠙⠻⢿⣿⣿⣿⣾⣿⣿⣿⣿⣟⣿⣿⣞⣿⣿⣟⣿⣯⣻⣿⣿⣿⣿⣏⣿⣿⣻⣿⣿⣻⣿⣿⡿⠟⠁
                                ⠈⠁⠁⠉ ⠈⠁ ⠈⠁ ⠈⠁ ⠁ ⠈⠁⠈⠈⠉⠉⠉⠉⠉⠉⠁
```

---

## 6. The Splash at Runtime (`opt/cli/splash.rs`)

The splash is printed by the **`udtk` command with no arguments**. The entry point is one check in `opt/cli/main.rs`:

```rust
// Filename: opt/cli/main.rs (abridged)
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        splash::print();
        return;
    }
```

`splash::print()` then does the following, in order.

### 6.1 Reading the Console

`console()` asks Windows for the output handle, reads the width and height from `GetConsoleScreenBufferInfo`, and decides whether colors are allowed. Colors are allowed only when the console really accepted the two settings it needs:

```rust
// Filename: opt/cli/splash.rs (console, abridged)
if GetConsoleMode(handle, &mut mode) == 1 {
    // colour is only honest when the console really accepted VT:
    // a raw ESC on a host without it is garbage, not a gradient
    let vt = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) == 1;
    let cp = SetConsoleOutputCP(65001) == 1;
    if vt && cp {
        return (true, w, h);
    }
}
return (false, w, h);
```

Both results are checked, not ignored. Without VT processing the ESC bytes would print as garbage, and without UTF-8 the Braille would print as garbage. When stdout is redirected, the code falls back to the size of the reader's window and returns colors off.

### 6.2 Environment Overrides

`print()` reads these environment variables (`opt/cli/splash.rs`, lines 68–80):

- `NO_COLOR` forces colors off when it is set to a non-empty value.
- `FORCE_COLOR` or `CLICOLOR_FORCE` forces colors on (useful for `less -R`).
- `COLUMNS` and `LINES` override the size that was read from the console.

### 6.3 Tiers from the Art's Own Width

The width thresholds are not constants. They come from the geometry of the art:

```rust
// Filename: opt/cli/splash.rs (thresholds, abridged)
const XL_MIN: usize = art::EMBLEM_XL_GEO.cols + GAP.len() + WORDMARK_COLS + 1;
const WIDE_MIN: usize = art::EMBLEM_WIDE_GEO.cols + GAP.len() + WORDMARK_COLS + 1;
const COMPACT_MIN: usize = art::EMBLEM_SMALL_GEO.cols + GAP.len() + WORDMARK_COLS + 1;
const STACK_MIN: usize = art::EMBLEM_SMALL_GEO.cols + 2;
```

`GAP` is three spaces and `WORDMARK_COLS` is `46`. So `XL_MIN` is `48 + 3 + 46 + 1 = 98`, `WIDE_MIN` is `86`, and `COMPACT_MIN` is `78`. A regenerated emblem therefore moves its own thresholds, and it cannot silently overflow a window it was not designed for.

`emblem_layout()` then picks the first tier that fits both the width and the row budget:

```rust
// Filename: opt/cli/splash.rs (emblem_layout, abridged)
fn emblem_layout(color: bool, width: usize, budget: usize) -> Vec<String> {
    if width >= XL_MIN && art::EMBLEM_XL.len() <= budget {
        return side_by_side(&art::EMBLEM_XL, &art::EMBLEM_XL_GEO, color, true);
    }
    if width >= WIDE_MIN && art::EMBLEM_WIDE.len() <= budget {
        return side_by_side(&art::EMBLEM_WIDE, &art::EMBLEM_WIDE_GEO, color, true);
    }
    if width >= COMPACT_MIN && art::EMBLEM_SMALL.len() <= budget {
        return side_by_side(&art::EMBLEM_SMALL, &art::EMBLEM_SMALL_GEO, color, true);
    }
    // ... narrower windows: stacked emblem, then text only ...
}
```

Below `STACK_MIN` (`28 + 2 = 30` columns) the splash is text only. Between that and `COMPACT_MIN`, the emblem goes above the wordmark, and each step removes one decoration until the text fits.

### 6.4 The Row Budget

The splash must not scroll, so it limits its height:

```rust
// Filename: opt/cli/splash.rs (rows_budget)
fn rows_budget(height: usize) -> usize {
    height.saturating_sub(2).clamp(10, 28)
}
```

Two rows are kept for the prompt. The result is limited to `28`, and the code also keeps at least `10`. The command map (section 6.5) takes its lines from the same budget, and the emblem gets what is left.

### 6.5 The Command Map

The commands are grouped as `control`, `telemetry`, `ui` and `system`, the same way as in the help text. Each row starts at column `13` (`CMD_INDENT`). In a narrow window a row wraps, and the continuation lines are indented to the same column, so no line overflows even at `COLUMNS=30`.

### 6.6 Colors

Colors are truecolor ANSI, computed from the emblem's geometry, not stored per cell. `emblem_color(col, row, rows, geo)` decides, for each cell, whether it belongs to the snake, the planet, or a ring, and then picks a colour from that zone's gradient. The palette comes from the constants at the top of `splash.rs`:

- The serpent (`BODY_TOP`, `BODY_BOTTOM`): titanium, `#C3D0DE` to `#5A6678`.
- The planet (`PLANET_TOP`, `PLANET_BOTTOM`): icy blue, `#C6EEFF` to `#367CB0`.
- The rings (`RING_TOP`, `RING_BOTTOM`): light blue, `#D6F4FF` to `#6CB2DA`.
- The lettering uses chrome and the colours of the German flag. The command group names use the accent `#d71921`.

### 6.7 The Photo Texture on the Serpent

Over the geometry, every filled cell of the snake's body is modulated by its `shade` byte from `splash_art.rs`. The function is `modulate(c, shade)`. `128` is neutral, so the planet, the rings and the background are never coloured by the photograph. The other values are quantized in steps of `8` within `80…176`, which is the range the generator enforces (section 4.7).

The shape stays procedural: the silhouette, the scales, the jaw and Saturn all come from the geometry. Only the light transitions along the body come from the photograph. Identical bytes give an identical colour, so `NO_COLOR` output stays a clean silhouette with no dithering noise.

### 6.8 Tiers, Heights and What Was Not Re-measured

The old text listed a height for each tier: XL at 29 rows and up, wide at 22 splash lines and compact at 18. The code does not gate on a fixed height. It gates on the row budget from section 6.4. So the height that a tier needs depends on how many command lines there are. The splash line counts quoted in the old text were not re-measured here.

---

## 7. Auditing the Live Output

Passing the generator's checks does not prove that the binary prints the right bytes. A separate tool reads the capture of a real run. You need a raw capture, because a PowerShell `>` redirect breaks UTF-8. The capture is taken with the environment variables from section 6.2 set, and then passed to `audit_capture.py`:

```bash
# capture raw output: Start-Process writes bytes, the PS > operator breaks UTF-8
FORCE_COLOR=1 COLUMNS=120 LINES=30  udtk > capture.rgb     # for example through Start-Process -RedirectStandardOutput
"$PY" ".../scripts/audit_capture.py" capture.rgb --width 120 --height 30
"$PY" ".../scripts/audit_capture.py" plain.txt --plain      # NO_COLOR: the grid is identical to the art
"$PY" ".../scripts/preview_splash.py" capture.rgb out.png   # render back to PNG for a visual audit
```

`audit_capture.py` checks four things:

1. **No stray escapes.** The script searches for an ESC byte that is not the start of an SGR colour code. It reports `FAIL: stray escapes` if it finds one.
2. **Fit.** The output has at most `min(28, height − 2)` lines, and no line is wider than the window.
3. **Tier.** The script works out which tier was printed and compares it with the width that was passed in.
4. **Colour.** Every filled cell carries exactly the colour that the geometry, the row gradient and the shade byte predict. The check uses an f32 match, including rounding half up, which matches how Rust rounds.

With `--plain`, the capture must be the same glyph grid as the art, with no escapes at all. `preview_splash.py` renders the capture back to an image, so a person can look at it.

---

## 8. Source Painting for the Photo Texture

* **File:** `Serpiente alquimica.jpg`, an Ouroboros from a Byzantine alchemical manuscript codex (a copy from 1478, Paris, Greek 2327).
* **License:** **Public domain** (PD-old: the work is over 100 years old, no attribution required).
* **Page:** <https://commons.wikimedia.org/wiki/File:Serpiente_alquimica.jpg>
* **SHA-256:** `af419b614c4bc60d2ec70f88d6a2c3a905ef003a050869bf68626c15ec9e3109` (checked against the file in the skill folder for this rewrite).
* **Storage:** `C:\Users\admin\Documents\skills\ascii-art\assets\serpiente.jpg`, **outside the repository**. The picture never enters git or the distribution. Only the text art goes into the repository.
* **How it is used:** a mask (the snake's red and olive paint against the parchment: `hue<25 | hue>330` with `S>0.35`, or `hue 58…120` with `S>0.30`) → the ring is unwrapped into polar coordinates `(θ, v)` using the `5/95` percentiles of the radius at each angle → two smoothing convolutions → normalization by the median and MAD → quantization into bytes `80…176` in steps of `8` → resampling onto the snake's body. Saturn and the rings receive no texture. The generator uses the same sphere and ellipse test as `emblem_color()` at runtime.

---

## 9. Reproduction Recipe (done)

1. **The exact vector path is implemented.** Instead of hand-drawn "SVG by eye", the geometry is computed by code: a regular hexagon with rounded edges, a variable-thickness brush for the body, a separate skull polygon with an open jaw, and a sphere with rings and a terminator.
2. **Generator:** `C:\Users\admin\Documents\skills\ascii-art\scripts\unidtk_emblem.py` (next to the skill, **outside the repository**).
   * The only external dependency is the skill's environment (`numpy`, `Pillow`). The project stays plain Rust/Tauri/JS.
   * Key parameters of the reference: `dots_w=220`, `dots_h=188`, `R=84`, `rc=20`, `body_half=6.2`, `head_scale=1.2`, `sphere_r=36.5`, `tilt_deg=26.7`, rings `a/b = 66/17.6, 59/15.7, 51.5/13.7`, `shade=0.85`, binarization threshold `0.40`.
   * Body scales: `scale_pitch=6.8`, `scale_depth=2.05`, `scale_w=1.15`, `scale_inset=1.0`, `scale_min_half=2.6` (on the thin tail the scales are not cut). Head scales: rows `u = 1.5 / 6.5 / 11.5`, `head_scale_bow=2.0`.
3. **Command (full cycle: vector → dot grid → Braille → edge cropping):**

   ```bash
   PY="C:/Users/admin/Documents/skills/ascii-art/scripts/.venv/Scripts/python.exe"
   cd <output dir>
   "$PY" "C:/Users/admin/Documents/skills/ascii-art/scripts/unidtk_emblem.py" \
       --out emblem.png --raw emblem_raw.png --braille 110,88
   ```

   Output: `emblem.png` (dot grid `220×188`), `emblem_raw.png` (the vector render, for auditing the shape), and the text blocks `ascii/unidtk_emblem_110.txt` and `ascii/unidtk_emblem_88.txt`. Empty Braille dots (`U+2800`) are replaced with plain spaces, so the repository contains no invisible characters. (Those two text files are not in the skill folder in this tree; see section 10.)
4. **The manual "minimalism path" is closed.** The complex organic anatomy still reads at a small scale (`88` cells), so the simplified sign was dropped in favor of the single emblem.
5. **The CLI splash data is generated, not copied by hand.** `make_splash.py` (next to the skill, **outside the repository**) is the single source of `opt/cli/splash_art.rs`:

   ```bash
   PY="C:/Users/admin/Documents/skills/ascii-art/scripts/.venv/Scripts/python.exe"
   "$PY" "C:/Users/admin/Documents/skills/ascii-art/scripts/make_splash.py" \
       --repo "C:/Users/admin/Documents/GitHub/shell"          # example path from the old text: point --repo at your UniDTK checkout
   "$PY" ".../make_splash.py" --repo <repo> --check             # check the file still matches the generator
   ```

   What it does: it builds a procedural emblem (`R=84`, `rc=20`, the hexagon from section 4.1, a `200×200` grid, threshold `0.35`) → binarizes it → builds the photo-texture field from `assets/serpiente.jpg` (section 8) → runs the grid through the skill at widths `48 / 36 / 28` → crops the empty lines, remembering `top_off` → computes the scales (`scale_x = scale_y = dots/(2·cols)`, where `scale_y` uses the **uncropped** line count) → writes the grids, `shade`, and the geometry into `splash_art.rs`.

   Built-in audits (the generator fails if anything is wrong): the width of each line `== geo.cols == length of the shade row`, the alphabet is only `space` + `U+2801…U+28FF`, a neutral/quantized `shade`, the presence of the texture, and a **zone check** (section 4.7). The format is deterministic: a repeated run produces the same file, byte for byte. (`--check` returns a non-zero exit code if `splash_art.rs` was edited by hand.)

6. **Auditing the live output is a separate tool, not "looks fine in the code".** `audit_capture.py` compares the bytes the binary actually printed with the data in `splash_art.rs` and with the color rule (section 7).
7. **Distribution rule:** the art enters the repository **only** as static UTF-8 text (`README.md`, `docs/`, `opt/cli/splash_art.rs`). The generators, the source picture, and the Python environment are not part of the distribution and are not committed to git. That is why the provenance (URL, SHA-256, license) is recorded in section 8 and in the header of `splash_art.rs` itself.

---

## 10. Known Gaps

These are the places where the old text and the current tree differ. They are listed so that they can be fixed in the code or in the text.

1. **Compact emblem height.** The compact tier is `28 × 13` (`EMBLEM_SMALL` has 13 rows, and its shade array has 364 bytes), not `28 × 12`.
2. **Text-only threshold.** The text-only mode starts below `30` columns (`STACK_MIN = 28 + 2`), not `29`.
3. **Wordmark.** The splash uses its own six-row `WORDMARK` in `splash_art.rs`. It is not the same block as section 5.1, and the old text was wrong to say that it was.
4. **Planet terminator in the splash.** The docstring of `make_splash.py` lists an ordered terminator dither in the pipeline. Its configuration sets the planet's shade to `0.0` and says `No Bayer terminator on the planet`. So the splash has no dithered terminator, and only the vector emblem in section 4.2 has one.
5. **Splash scale parameters.** `make_splash.py` uses its own parameters (`SCALE_PITCH = 11.0`, `scale_depth=3.2`, `scale_w=3.0`). The `6.8` and `2.05` values in section 3 and section 9 belong to `unidtk_emblem.py`, not to the splash generator.
6. **Missing text outputs.** `ascii/unidtk_emblem_110.txt` and `ascii/unidtk_emblem_88.txt` are named by the recipe but are not in `C:\Users\admin\Documents\skills\ascii-art\scripts\ascii` in this tree.
7. **Not re-run for this rewrite.** The zone agreement figure (`97…99%`), the splash line counts (`27` / `22` / `18`), and the byte counts of the live capture were recorded in the old text. They were not re-measured here.
