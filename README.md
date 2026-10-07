# Leo

An AI assistant with a living 3D orb, for Windows. Chat with it, let it search the web, remember things about you, and work with your Gmail, Calendar, Drive, Docs, Sheets, Slides and Contacts. It always asks before it changes anything.

## Install

1. Open the [latest release](https://github.com/Priyanshu-Madhup/Leo-AI/releases/latest).
2. Download `Leo_..._x64-setup.exe` and run it. No administrator rights are needed.
3. Open Leo. A settings window appears: paste an [OpenRouter key](https://openrouter.ai/keys) and pick a model that supports tool use. Everything else is optional.

Leo updates itself: when a newer release is published, the next time you open the app it downloads and installs it quietly and restarts.

### Optional connections (Settings)

| Feature | What you need |
|---|---|
| AI model | OpenRouter API key and a model name (required) |
| Long-term memory | MemoryLake API key |
| Web search | Tavily API key |
| Google (Gmail, Calendar, Drive, ...) | Your Google email in Settings; Leo opens a Google sign-in in your browser the first time it needs it |

Each setting has a "Get a key" link next to it.

## Build from source

```
npm install
npm run tauri dev       # run the app
npm run tauri build     # build the installer (needs the signing key, see below)
```

Needs Node 20+, Rust (stable) and the Tauri prerequisites for Windows. The project's full technical guide is in [detailed_readme.md](detailed_readme.md).

## Releasing a new version

1. Raise `version` in `src-tauri/tauri.conf.json` (and keep `package.json` / `Cargo.toml` in step).
2. Push to `main`.

The **Release** workflow builds the installer and publishes it as a GitHub release together with `latest.json`, which installed apps check on launch. A push to `main` without a new version does nothing.

### One-time repository setup

Add these under *Settings → Secrets and variables → Actions*:

| Secret | What it is |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | The updater signing key (`tauri signer generate`); the matching public key is in `tauri.conf.json`. Required. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Its password (leave empty if none). |
| `LEO_GOOGLE_CLIENT_ID`, `LEO_GOOGLE_CLIENT_SECRET` | The Google OAuth Desktop client baked into released builds so users can sign in with Google. Optional: without them the Google connection is simply not preconfigured. |

Never commit keys. Losing the signing key means installed apps can no longer be updated.
