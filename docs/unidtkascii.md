# ASCII Graphics Research and Concept for UniDTK (`docs/unidtkascii.md`)

> **Purpose of this document:** Records the requirements, concepts, experiments, technical constraints, and drafts for the ASCII/Unicode identity and the graphic logo of the `UniDTK` project.

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
   * The brand header art at the top of the repository docs, **shipped** (the compact `88`-cell variant, see `4.5`).
   * A tree of the file structure (`core/`, `modules/`, `opt/`, `toolchain/`).
2. **Architecture diagrams (`docs/`):**
   * Diagrams of the system layers (core, event bus, Tauri/CLI/AI bridges, the `unidtk-api` contract).
   * Diagrams of the split between `Widgets` and `Surfaces` objects (`unicoremodmotion.md`).
   * Diagrams of the life cycle and of the 10-step development cycle in `AGENTS.md`.
3. **The `udtk` CLI tool (`opt/cli`):**
   * A splash screen when run **with no arguments** (`udtk`), **shipped**: the emblem on the left, the block lettering `UNIDTK` on the right, and below it a map of the commands in plain text, all in color (see `4.6`). `udtk help` / `-h` / `--help` deliberately stay plain, uncolored text, so their output remains usable in scripts.
   * A header for the system diagnostics `udtk doctor` and the `udtk status` summary, **not shipped**.

> [!IMPORTANT]
> **Architecture rule:** `UniDTK` is written in plain Rust/Tauri/JS and **must not depend on Python or third-party graphics libraries under any circumstances**. All final ASCII graphics are stored only as static UTF-8 text in the code and the docs.

---

## 3. Technical Pitfalls and Lessons Learned

A series of practical experiments revealed the main difficulties in generating complex figurative ASCII graphics:

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

## 4. Recorded Variants and Prototypes

> **Status:** variants `4.2` and `4.3` are unimplemented sketches from earlier passes (broken lines, lost proportions). Variant `4.4` is the **final verified art**, and it is the reference for the identity.

### 4.1 Base Lettering Block `UNIDTK` (verified, clean)

```text
██╗   ██╗███╗   ██╗██╗██████╗ ████████╗██╗  ██╗
██║   ██║████╗  ██║██║██╔══██╗╚══██╔══╝██║ ██╔╝
██║   ██║██╔██╗ ██║██║██║  ██║   ██║   █████╔╝ 
██║   ██║██║╚██╗██║██║██║  ██║   ██║   ██╔═██╗ 
╚██████╔╝██║ ╚████║██║██████╔╝   ██║   ██║  ██╗
 ╚═════╝ ╚═╝  ╚═══╝╚═╝╚═════╝    ╚═╝   ╚═╝  ╚═╝
```

---

### 4.2 Prototype: Compact Cyber Symbol of the Ouroboros

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

### 4.3 Prototype: Detailed Figurative Sketch

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

### 4.4 Final `UNIDTK` Emblem (reference, 110 cells)

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

### 4.5 Compact Variant for `README.md` (88 cells)

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

### 4.6 CLI Splash `udtk` (implemented, `opt/cli/splash.rs`)

It is printed by the **`udtk` command with no arguments**. It is sized to fit the whole terminal without scrolling:

* **Left:** the Ouroboros emblem. **Right:** the block lettering `UniDTK` (`4.1`), with the caption `Unified Desktop Täglich Kit` and a stack line. The columns are chosen by area, so the lettering does not look smaller than the logo.
* **Bottom:** the command map in plain text, grouped the same way as in the help: `control` / `telemetry` / `ui` / `system`. In narrow windows the command list **wraps** with an indent of `13` columns, so a line does not overflow even at `COLUMNS=30`.
* **Color:** truecolor ANSI. The emblem's coloring is computed from its geometry in dot space (`emblem_color`). The snake has a titanium gradient `#C3D0DE→#5A6678`, the planet an icy `#C6EEFF→#367CB0`, and the rings light `#D6F4FF→#6CB2DA`. The lettering uses chrome plus the colors of the German flag. The command group names use the accent `#d71921`.
* **Photo texture on top of the brand** (`4.7`). Over the geometry, every filled cell of the snake's body is multiplied by the byte `shade` from `splash_art.rs`. This is the brightness taken from the source painting of the Ouroboros. `128` is neutral (the planet, the rings, and the background are never colored), and the other values are quantized in steps of `8` within `80…176`. The shape stays procedural (silhouette, scales, jaw, Saturn), while the scale relief and the light transitions come from the photograph. Identical bytes give an identical color, byte for byte, so `NO_COLOR` output stays a clean silhouette with no dithering noise.
* **Tiers by width and height** (the thresholds are derived from the art's own width, `art_width + 3 + 46 + 1`, not from constants):
  * `≥98` columns **and** window height `≥29`: XL. The emblem is `48` columns × `21` lines, the splash `27` lines.
  * `≥86`: wide. `36` × `16`, splash `22` lines.
  * `78–85`: compact. `28` × `12`, splash `18` lines.
  * `<78`: the emblem above the lettering, with a ladder of truncations (caption → empty line → the `UniDTK` line instead of the block lettering when the width is `<46`).
  * `<29` columns: a text mode with no art.
* **Line budget:** `min(28, height − 2)`. Two rows are left for the prompt. The height is read from `srWindow` in the same `GetConsoleScreenBufferInfo` call as the width, and can be overridden by the `LINES` variable (symmetric with `COLUMNS`). Because of this, the splash does not scroll even in `24`-line windows. The old stacked layout took `26` lines and scrolled.
* **When colors are off:** output to a file or a pipe turns colors off. `NO_COLOR` forces them off, and `FORCE_COLOR` / `CLICOLOR_FORCE` force them on (useful for `less -R`). `COLUMNS` / `LINES` override the geometry. Color is turned on **only** when the console has actually accepted VT processing and the UTF-8 code page. The results of `SetConsoleMode` / `SetConsoleOutputCP` are checked, not ignored. Otherwise raw ESC bytes would reach the console as garbage. In a real console the splash enables VT and CP65001. Without this, the Braille would print as garbage.

---

### 4.7 Source Painting for the Photo Texture

* **File:** `Serpiente alquimica.jpg`, an Ouroboros from a Byzantine alchemical manuscript codex (a copy from 1478, Paris, Greek 2327).
* **License:** **Public domain** (PD-old: the work is over 100 years old, no attribution required).
* **Page:** <https://commons.wikimedia.org/wiki/File:Serpiente_alquimica.jpg>
* **SHA-256:** `af419b614c4bc60d2ec70f88d6a2c3a905ef003a050869bf68626c15ec9e3109`
* **Storage:** `C:\Users\admin\Documents\skills\ascii-art\assets\serpiente.jpg`, **outside the repository**. The picture never enters git or the distribution. Only the text art goes into the repository.
* **How it is used:** a mask (the snake's red and olive paint against the parchment: `hue<25 | hue>330` with `S>0.35`, or `hue 58…120` with `S>0.30`) → the ring is unwrapped into polar coordinates `(θ, v)` using the `5/95` percentiles of the radius at each angle → two smoothing convolutions → normalization by the median and MAD → quantization into bytes `80…176` in steps of `8` → resampling onto the snake's body. Saturn and the rings receive no texture. The generator uses the same sphere and ellipse test as `emblem_color()` at runtime.

## 5. Reproduction Recipe (done)

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

   Output: `emblem.png` (dot grid `220×188`), `emblem_raw.png` (the vector render, for auditing the shape), and the ready-made text blocks `ascii/unidtk_emblem_110.txt` and `ascii/unidtk_emblem_88.txt`. Empty Braille dots (`U+2800`) are replaced with plain spaces, so the repository contains no invisible characters.
4. **The manual "minimalism path" is closed.** The complex organic anatomy still reads at a small scale (`88` cells), so the simplified sign was dropped in favor of the single emblem.
5. **The CLI splash data is generated, not copied by hand.** `make_splash.py` (next to the skill, **outside the repository**) is the single source of `opt/cli/splash_art.rs`:

   ```bash
   PY="C:/Users/admin/Documents/skills/ascii-art/scripts/.venv/Scripts/python.exe"
   "$PY" "C:/Users/admin/Documents/skills/ascii-art/scripts/make_splash.py" \
       --repo "C:/Users/admin/Documents/GitHub/shell"          # write the art
   "$PY" ".../make_splash.py" --repo <repo> --check             # check the file still matches the generator
   ```

   What it does: it builds a procedural emblem (`R=84`, `rc=20`, the hexagon from §1, a `200×200` grid, threshold `0.35`) → binarizes it and applies the planet's dithered terminator → builds the photo-texture field from `assets/serpiente.jpg` (`4.7`) → runs the grid through the skill at widths `48 / 36 / 28` → crops the empty lines, remembering `top_off` → computes the scales (`scale_x = scale_y = dots/(2·cols)`, where `scale_y` uses the **uncropped** line count) → writes the grids, `shade`, and the geometry into `splash_art.rs`.

   Built-in audits (the generator fails if anything is wrong): the width of each line `== geo.cols == length of the shade row`, the alphabet is only `space` + `U+2801…U+28FF`, a neutral/quantized `shade`, the presence of the texture, and a **zone check**. The "planet / rings / body" decision at each cell center is compared with the ground truth for each Braille dot, giving `97…99%` agreement, with all three zones present. The format is deterministic: a repeated run produces the same file, byte for byte. (`--check` returns a non-zero exit code if `splash_art.rs` was edited by hand.)

6. **Auditing the live output is a separate tool, not "looks fine in the code".** `audit_capture.py` compares the bytes the binary actually printed with the data in `splash_art.rs` and with the color rule:

   ```bash
   # capture raw output: Start-Process writes bytes, the PS > operator breaks UTF-8
   FORCE_COLOR=1 COLUMNS=120 LINES=30  udtk > capture.rgb     # for example through Start-Process -RedirectStandardOutput
   "$PY" ".../scripts/audit_capture.py" capture.rgb --width 120 --height 30
   "$PY" ".../scripts/audit_capture.py" plain.txt --plain      # NO_COLOR: the grid is identical to the art
   "$PY" ".../scripts/preview_splash.py" capture.rgb out.png   # render back to PNG for a visual audit
   ```

   It checks: there are no stray ESC bytes, the output fits in `min(28, height−2)` lines and the window width, the tier is detected correctly, and **every** filled cell carries exactly the color promised by the geometry, the row gradient, and the `shade` byte. (The last check is an f32 match, including rounding half up, as Rust does.)
7. **Distribution rule:** the art enters the repository **only** as static UTF-8 text (`README.md`, `docs/`, `opt/cli/splash_art.rs`). The generators, the source picture, and the Python environment are not part of the distribution and are not committed to git. That is why the provenance (URL, SHA-256, license) is recorded in `4.7` and in the header of `splash_art.rs` itself.
