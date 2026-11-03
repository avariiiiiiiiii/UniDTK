# AI DJ: What Was Researched, and What the Code Does Now

The AI DJ is the mode of the music player that chooses the next track from its sound, not only from its order. The first version of this document was a research note with a plan. This version has two parts. Part 1 keeps the research as it was written, with a status for each item. Part 2 describes the pipeline the code implements now, step by step, with the code as the source. Part 3 compares the two.

Facts in Part 2 are taken from the code. Facts in Part 1 that come from the research notes (licences, parameter counts, the original claims) are marked as research and were not re-checked for this rewrite.

## What you will understand by the end

- How an audio file becomes a 400-number embedding, and what else is measured from it.
- How the model is found on disk, and how it is downloaded and verified.
- How the next track is chosen from the similarity graph and from tempo and key.
- Which parts of the research were built, which were changed, and which were not built.
- Why the default model download does not work at the moment, and what to do about it.

## Files involved

| File | Role |
| :--- | :--- |
| [modules/mp3-player/rust/ai/ai_dj.rs](../../modules/mp3-player/rust/ai/ai_dj.rs) | Analysis, embeddings, the similarity graph, next-track scoring, the model search, ONNX inference (1,624 lines). |
| [modules/mp3-player/rust/ai/model_fetch.rs](../../modules/mp3-player/rust/ai/model_fetch.rs) | Downloading the model with retries, resume, and SHA-256 records (1,639 lines). |
| [modules/mp3-player/rust/commands.rs](../../modules/mp3-player/rust/commands.rs) | The analysis loop (`run_analysis_loop`) and the AI DJ commands. |
| [modules/mp3-player/rust/playback/player.rs](../../modules/mp3-player/rust/playback/player.rs) | Enabling the AI DJ, the modes, and the end-of-track advance. |
| [modules/mp3-player/ui/widget.js](../../modules/mp3-player/ui/widget.js) | The `AI DJ` mode button. |
| [modules/mp3-player/rust/Cargo.toml](../../modules/mp3-player/rust/Cargo.toml) | The libraries the code uses: `ort`, `symphonia`, `rustfft`, `ureq`, `sha2`. |
| [opt/models/discogs-maest-129e/](../../opt/models/discogs-maest-129e) | A local export of the model, with the Python script that produced it. |

---

## Part 1: The research

The research compared several models and libraries, and chose one model. The table shows each candidate and what happened to it. Numbers and licences are from the research notes.

### 1.1 Models

| Model | Source (from the research) | Research notes | Status in the code |
| :--- | :--- | :--- | :--- |
| Discogs-MAEST, 129 M parameters (`discogs-maest-129e`) | [mtg-upf/discogs-maest-30s-pw-129e](https://huggingface.co/mtg-upf/discogs-maest-30s-pw-129e) | Audio Spectrogram Transformer; 416-dimensional embedding per 30 s; CC-BY-NC-SA-4.0; trained on Discogs tags | **Used.** The code expects 400 dimensions (`EMBEDDING_DIM = 400`), not 416. |
| Discogs-MAEST, 73 M (tiny) and 519 M (large) | same family | Smaller and larger variants | Not used. |
| MusicNN | [oriyonay/musicnn-pytorch](https://huggingface.co/oriyonay/musicnn-pytorch) | About 8 M parameters; tag prediction | Not used. |
| Music sentiment | [nikarpoff/music-sentiment-analysis-happy-sad-13M](https://huggingface.co/nikarpoff/music-sentiment-analysis-happy-sad-13M) | Mood only | Not used. |
| Genre classification | [dima806/music_genres_classification](https://huggingface.co/dima806/music_genres_classification) | DistilHuBERT, about 6 M parameters | Not used. |

The research picked `discogs-maest-129e` for its embeddings, at roughly 200 MB more than the 73 M variant. The local export in `opt/models/discogs-maest-129e/` has two files that matter:

| File | Size in bytes | Role |
| :--- | :--- | :--- |
| `model.onnx` | 122,191 | The graph. |
| `model.onnx.data` | 347,471,872 | The external weights. This is 331.4 MiB, which is the "~331 MB" in the code's error text. |

The folder also contains `feature_extraction_maest.py` and `preprocessor_config.json`, the export and preprocessing references.

### 1.2 Libraries

| Library | Research purpose | Status in the code |
| :--- | :--- | :--- |
| `symphonia` (pure Rust audio decoder) | Decode MP3, FLAC, OGG, WAV | **Used.** Version 0.5, with the mp3, flac, ogg, wav, aac, isomp4, alac, and vorbis features. |
| `rustfft` | Spectral analysis | **Used.** Version 6.4 for the FFT. |
| `mel_spec` | Mel spectrogram | Not used. The code has its own mel filterbank (`create_mel_filterbank`). |
| `ort` (ONNX Runtime bindings) | Run the exported model | **Used.** Version `2.0.0-rc.13`. |
| `fundsp` | DSP building blocks | Not used. |
| `whisper-rs` | Speech and vocal detection | Not used. |

### 1.3 Storage

The research proposed a redb database with two tables: `embeddings` (key: track ID, value: 416 floats as bytes) and `metadata` (JSON with BPM, key, duration, and analysis time). It estimated 1.6 KB per track and about 16 MB for 10,000 tracks.

The code does not use its own redb tables. It stores its data in the module's store, which the kernel provides for the module under the name `ai_dj`. Keys are `emb:<track id>` for embeddings and `ana:<track id>` for analysis (BPM and key). A 400-float vector is 1,600 bytes before any encoding overhead.

### 1.4 The proposed phases

| Phase (research) | What it proposed | Status |
| :--- | :--- | :--- |
| 1. Feature extraction | Decode, mel spectrogram, ONNX model, 416-dimensional vector | **Built** (400-dimensional). See Part 2, steps 3 to 5. |
| 2. Similarity graph | Cosine similarity, nodes and edges | **Built.** See Part 2, step 8. |
| 3. Transition path | Score neighbours by similarity, BPM, Camelot key, and an energy curve; Dijkstra for paths | **Partly built.** Similarity, BPM, and key are used. The energy curve is accepted as a parameter but not used. Dijkstra is not implemented. See Part 2, steps 9 and 10. |
| 4. AI DJ runtime | Choose the next track and crossfade at a beat-matched point | **Partly built.** The next track is chosen and the player advances. The beat-matched crossfade is not built. See Part 2, step 11. |

---

## Part 2: What the code does now

This is the pipeline, in the order it runs.

### Step 1: Finding the model

`find_model_path` checks these places in order, and returns the first one that is a valid model:

1. The path in the environment variable `AI_DJ_MODEL_PATH`, if it is set.
2. `models/discogs-maest-129e/model.onnx` in the install directory. The installer is meant to put the model there.
3. `models/discogs-maest-129e/model.onnx` in the data directory (`%LOCALAPPDATA%\UniDTK\models`). The downloader writes there.
4. For a development run from a checkout: `opt/models/discogs-maest-129e/model.onnx` in a parent folder of the executable.

A file is valid when `is_valid_onnx_model` accepts it. A graph of 50 MB or more is accepted on its own, because its weights are inside it. A smaller graph needs a companion weights file next to it, named `model.onnx.data` or `model.data`, that is larger than 10 MB.

If no candidate is valid, the error says that the model or its weights were not found and gives the number of candidates it checked.

### Step 2: Downloading the model

If the model is missing, the download runs in `model_fetch.rs`. The download uses two files, in this order:

1. The graph, `model.onnx`, about 122 KB. It is fetched first. If it arrives, the source is known to answer.
2. The weights, `model.onnx.data`, about 347 MB.

Each file is written to a `.part` file first. A download that stops is resumed from the last byte with an HTTP range request, and the server's `Content-Range` header is checked. When a file is complete, its SHA-256 and size are written to `download_manifest.json` in the model folder, with the URL and a timestamp.

Each file gets up to five attempts (`MAX_ATTEMPTS`). Attempt 1 goes direct over HTTPS. Attempt 2 goes through the SOCKS proxy at `127.0.0.1:12908`, which is the VPN's port (see [the VPN chapter](vpn.md)). Later attempts try direct and then SOCKS. The code waits between attempts: 1, 2, 4, and then 8 seconds (the formula caps the wait at 16 seconds, which is never reached with five attempts).

The source is `https://huggingface.co/mtg-upf/discogs-maest-30s-pw-129e/resolve/main/`, followed by the file name. The environment variable `AI_DJ_MODEL_URL` replaces this base, with no rebuild needed.

**The default source does not work.** The code's own comment, in `model_fetch.rs`, says that this repository does not contain `model.onnx` or `model.onnx.data`, in its root or in its file list. The export was made locally, so the default URL returns 404, and the user gets an error until a working source is set with `AI_DJ_MODEL_URL` or the files are placed by hand. The comment records this as checked at runtime. It was not checked again for this rewrite, because that needs network access.

### Step 3: Decoding the first 30 seconds

`decode_audio_samples_30s` uses `symphonia` to probe the file, decode its first track, and convert the samples to `f32`. If the source sample rate is not 16,000 Hz, it resamples by linear interpolation. The result is capped at 30 seconds, which is 480,000 samples at 16 kHz.

### Step 4: The mel spectrogram

`extract_analysis` calls `compute_mel_spectrogram(samples, 16000, 96, 512, 256, 1876)`. The arguments are:

| Argument | Value | Meaning |
| :--- | :--- | :--- |
| sample rate | 16,000 Hz | The rate the model expects. |
| mel bands | 96 | The height of the spectrogram. |
| window | 512 samples | The FFT size. The FFT is `rustfft`. |
| hop | 256 samples | The step between windows. |
| target frames | 1,876 | The number of time frames the model input gets. |

The filterbank is built by `create_mel_filterbank`, which is the code's own implementation.

### Step 5: Inference

`run_onnx_inference` creates the ONNX session once, and keeps it in a static `Mutex` for later calls. The input tensor is named `input_values` and has the shape `[1, frames, 96]`. The output named `logits` is used, or the first output if there is no output with that name. The result is returned as a vector of `f32`, and the code calls it a 400-dimensional embedding.

### Step 6: Tempo and key

The same 16 kHz samples are used for two more measures:

- `detect_bpm` estimates the tempo in beats per minute.
- `detect_key` estimates the musical key, written in Camelot notation (a number from 1 to 12 and a letter A or B).

`camelot_compatible(key1, key2)` says whether two keys are harmonically compatible on the Camelot wheel. Identical keys are always compatible.

### Step 7: The analysis job

`run_analysis_loop` in `commands.rs` runs over the library's tracks, one at a time. For each track it calls `extract_analysis`, and it stores the embedding under `emb:<id>` and the BPM and key under `ana:<id>`. While it runs, it sends a `ai-dj-progress` event at most every 150 milliseconds, with the current index, the total, and the track name. Two command paths start this loop.

Each stored embedding bumps an analysis revision counter, which invalidates the graph cache (step 8).

### Step 8: The similarity graph

`cosine_similarity` gives the similarity of two embeddings, from -1 to 1. `find_similar(track, k)` returns the `k` tracks most similar to the given one.

`build_similarity_graph(tracks, k)` builds a graph in which each track keeps its `k` nearest neighbours. The embedding norms are computed once, and each track's top-k list is kept by insertion, so the work does not grow with a sort over all tracks.

The graph is cached per store. The cache key combines a fingerprint of the track list with the analysis revision, so the graph is rebuilt only when the tracks or their embeddings change.

### Step 9: Choosing the next track

`next_track_scoped` chooses the next track. It works in two situations.

**With a playlist as the pool.** The candidates are the other tracks of the playlist, except those in the exclusion list. If every track has been played, the pool is used again without the current track (a circular pool). Each candidate's similarity to the current track is its cosine similarity. If an embedding is missing, the value is 0.5.

**Without a pool.** The candidates are the top 50 neighbours of the current track. The search widens in three steps when the list runs out:

1. top 50, minus the exclusions,
2. if empty, top 200, minus the exclusions,
3. if empty and there are exclusions, top 100 minus the last 15 history entries,
4. if still empty, any analysed tracks, up to 10, with a similarity of 0.5.

**Scoring.** Each candidate gets the score below, and the highest score wins:

- Its cosine similarity to the current track, as the base.
- A BPM bonus: +0.15 if the tempo differs by 5 BPM or less, +0.08 if by 10 or less, +0.02 if by 20 or less.
- A key bonus: +0.10 if the Camelot keys are compatible.

The function takes an `energy_curve` argument, but it is not used. The variable has an underscore prefix in the code to show this.

### Step 10: Ordering a playlist

`build_playlist_queue` orders a list of tracks into a queue. The caller gives the first track, a random seed, and the function then picks each following track from the remaining ones. Its doc comment calls this "greedy Hamiltonian pathfinding over embedding similarity, octave-aware BPM, and Camelot wheel compatibility". The loop itself is greedy. It does not run Dijkstra or search for an optimal path, and the research's `dijkstra_path` was not built.

### Step 11: How the player uses it

Enabling the AI DJ (`enable_ai_dj`) loads the module's `AiDj` object from the store and sets `ai_dj_enabled`. The player has three modes:

| Mode | What happens at the end of a track |
| :--- | :--- |
| Smart | `next()`, which asks `ai_dj_next_track` for the best-scored track. |
| Random | `ai_dj_play_random`, a random track from the library. |
| Sequential | `jump_relative(1)`, the next track in the queue. |

The end of a track is found by the end-of-track monitor described in [the libmpv chapter](mp3-player.md) (Step 7). The `AI DJ` button in the music widget ([modules/mp3-player/ui/widget.js](../../modules/mp3-player/ui/widget.js)) selects the mode.

A crossfade duration is stored in the player state and clamped to 0 to 10 seconds (`set_crossfade_duration`). No playback code reads it. The research's crossfade at a beat-matched point is not implemented.

---

## Part 3: Status against the research's next steps

The research ended with eight next steps, as a checklist. Here is the status of each one.

| # | Research step | Status | Where |
| :--- | :--- | :--- | :--- |
| 1 | Download and export discogs-maest to ONNX | **Done.** The export is in `opt/models/discogs-maest-129e/`. | `opt/models/` |
| 2 | Create the Rust module `ai_dj.rs` | **Done**, at a different path. | `modules/mp3-player/rust/ai/ai_dj.rs` |
| 3 | Implement the embedding extraction pipeline | **Done** (400 dimensions). | `extract_analysis` |
| 4 | Build the similarity graph | **Done.** | `build_similarity_graph`, `find_similar` |
| 5 | Implement the transition path (Dijkstra and heuristics) | **Partly done.** The heuristics are built. Dijkstra is not. Playlists use a greedy walk. | `next_track_scoped`, `build_playlist_queue` |
| 6 | Add SQLite storage for embeddings | **Not done as written.** The module store is used instead. No SQLite. | `ai_dj_store`, keys `emb:` and `ana:` |
| 7 | Integrate with `player.rs` for auto-advance | **Done.** | `next()` in Smart mode, and the end-of-track monitor |
| 8 | Add a UI toggle in player settings | **Done**, as a mode button in the music widget. | `widget.js` |

---

## Known issues

- **The default model source returns 404.** Set `AI_DJ_MODEL_URL`, or place `model.onnx` and `model.onnx.data` in `%LOCALAPPDATA%\UniDTK\models\discogs-maest-129e\`, or in the development folder `opt/models/`.
- **The weights size is written two ways.** `model_fetch.rs` says 347 MB, and `ai_dj.rs` says about 331 MB. The file is 347,471,872 bytes, which is 347 MB in decimal units or 331 MiB in binary units.
- **The embedding size differs from the research.** The research said 416; the code uses 400.
- **The energy curve is unused.** It is part of the signature but does nothing.
- **Dijkstra is not implemented.** Playlist ordering is greedy.
- **The crossfade setting does nothing.** It is stored and clamped but never applied.

---

## Summary

- Embeddings come from a 16 kHz mel spectrogram of the first 30 seconds, passed through an ONNX model with 400 outputs.
- Tempo and key are measured from the same samples, and used as bonuses in the score.
- The next track is the best-scored neighbour of the current one, with the search widened when history excludes too much.
- The model download has a default source that fails, and it can be replaced with an environment variable or by placing the files by hand.
- The research's Dijkstra path, energy curve, and beat-matched crossfade are not built.
