# Prototype backend contract

Observed through read-only requests to `http://localhost:9791` during prototype setup. This is a forwarded Speaches deployment, not a backend installed by this project.

## Verified read endpoints

| Endpoint | Observed result |
| --- | --- |
| GET `/health` | 200, plain text `OK` |
| GET `/openapi.json` | 200, OpenAPI schema |
| GET `/v1/models` | 200, `{ "data": [...], "object": "list" }` |

Listed STT models:

- `distil-whisper/distil-large-v3.5-ct2`
- `Systran/faster-whisper-medium.en`
- `Systran/faster-distil-whisper-large-v3`
- `Systran/faster-whisper-small.en`
- `deepdml/faster-whisper-large-v3-turbo-ct2`
- `Systran/faster-whisper-base.en`
- `Systran/faster-distil-whisper-medium.en`

The multilingual model above is the initial selection. Listing proves discovery, not successful inference.

## Transcription adapter

POST `/v1/audio/transcriptions`, multipart fields:

- `file`: mono PCM16 WAV, `recording.wav`, `audio/wav`
- `model`: selected ID
- `response_format`: `json`
- `stream`: `false`
- `language`: optional, omitted when blank

Expected response: `{ "text": "…" }`.

The live OpenAPI unusually declares URL-encoded content despite a binary file field. Multipart follows Speaches/OpenAI-compatible upload conventions; acceptance must be established through user-initiated live dictation. Mock-server tests verify the client request, not server inference.

The client preserves reverse-proxy prefixes and accepts a base URL with or without trailing `/v1`. It rejects embedded credentials, query strings, fragments, and non-HTTP(S) schemes. All redirects are disabled. Connection timeout is five seconds; discovery ten seconds; transcription 180 seconds. Bodies are bounded to 2 MiB. Server response bodies are not copied into user errors or logs.

## Deferred management

Model download/load/unload/delete controls are not implemented in this prototype. Before that V1 phase, verify each method/path/schema and supported-version behavior using authoritative Speaches documentation and the configured server's OpenAPI. Do not infer routes from OpenAI compatibility, probe through mutations, or equate unauthorized/unreachable with unsupported.
