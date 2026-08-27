# bench — ASR backend comparison

Published WER numbers are close to useless for this app. They come from
different test sets at different difficulty levels (Whisper's headline 2.8% is
LibriSpeech *clean* — studio-recorded audiobooks), under different metrics
(WER vs CER vs cpWER), and the big leaderboards are English-only.

This app captures loopback audio: streams, games, Discord — background music,
overlapping speakers, compressed audio, casual Korean with English loanwords.
Nothing published tells you how a model does on that.

So measure it on your own audio.

## Usage

```powershell
python bench/compare_backends.py "C:\clips\some_korean_stream.mp4" `
  --backends whisper-large,whisper-turbo,zipformer-ko --lang ko
```

Outputs `bench/out/comparison.md` — a timing table plus every segment's
transcript side by side, one column per backend.

### Dependencies

Only `numpy`, which `asr_srv.py` already pulls in. HTTP is stdlib, so there is
nothing extra to `pip install`.

A **16 kHz mono 16-bit WAV** is decoded natively and needs nothing else. For any
other input — mp4, mkv, mp3, stereo, a different sample rate — you need ffmpeg:

```powershell
winget install Gyan.FFmpeg
```

Then open a new terminal so PATH picks it up. Resampling is left to ffmpeg on
purpose: a naive resampler would degrade the audio for every backend at once and
quietly bias the whole comparison.

## Why it segments the audio itself

It reuses the graduated-silence rule from `pipeline/chunker.rs` (800 ms of
silence to cut under 1.5 s of buffer, down to 200 ms past 2.5 s, 6 s hard cap),
so each backend sees the same chunk boundaries the running app would send it.
Transcribing the whole file in one pass would measure something the app never
does — models behave differently on 3-second fragments than on 30-minute files.

Requests go through `asr_srv.py`'s real `/inference` endpoint, one server
process per backend, launched and torn down in turn.

## Backend aliases

| alias | model | notes |
|---|---|---|
| `whisper-turbo` | `deepdml/faster-whisper-large-v3-turbo-ct2` | float16, ~1.6 GB VRAM. Distilled 4-layer decoder — fast, weaker on non-English |
| `whisper-large` | `Systran/faster-whisper-large-v3` | int8_float16, ~1.5 GB VRAM |
| `whisper-large-fp16` | `Systran/faster-whisper-large-v3` | float16, ~3 GB VRAM, no quantization loss |
| `whisper-medium` | `Systran/faster-whisper-medium` | for comparison against the old default |
| `sensevoice` | sherpa-onnx SenseVoice int8 | zh/en/ja/ko/yue, CPU |
| `zipformer-ko` | Korean Zipformer transducer | Korean specialist, CPU |

Anything else is read as `backend:model`, e.g. `whisper:openai/whisper-large-v3`.

First run of a backend downloads its weights, which is why `--load-timeout`
defaults to 10 minutes. Use `--max-segments 10` for a quick smoke run.

## Reading the results

**Latency matters as much as accuracy here.** The app sends a rolling partial
1 s into an utterance and every 1.5 s after. A backend whose median inference
exceeds ~1.5 s gets its partials coalesced away by the backlog logic in
`asr/http_client.rs` — nothing breaks, but the live preview thins out. The
timing table reports median/p90/max for exactly this reason.

For accuracy, read the transcript columns rather than computing a WER against a
reference — you know what was actually said, and the failure modes that matter
(hallucinated subtitle credits during silence, repeat loops, dropped English
loanwords, wrong language detection) are obvious on sight and invisible in a
single aggregate number.

## Note on ports

Defaults to 9101 so it does not collide with the app's asr-srv on 9001. You can
leave the app running while benchmarking.
