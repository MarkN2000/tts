# TTS Server

VOICEVOX Engine / AivisSpeech Engine の音声合成 API を仲介し、生成した WAV を FFmpeg で Oggへ変換して公開する小さなサーバーです。

## 必要なもの

- 起動済みの VOICEVOX Engine または AivisSpeech Engine
- `libopus` または `libvorbis` が有効な FFmpeg
- ビルドする場合は Rust

FFmpeg は `config.toml` の `ffmpeg_path` で指定するか、`PATH` から実行できるようにしてください。

## ビルド

```console
cargo build --release
```

ビルド後、次の2ファイルを同じディレクトリへ配置します。

```text
tts-server.exe  # Linuxでは tts-server
config.toml
```

`config.toml` の `engine_url`、`public_base_url`、`default_id` などを環境に合わせて変更し、実行ファイルを起動してください。相対指定の `cache_dir` は、このディレクトリを基準にします。

## API

音声を生成します。`id` は省略でき、存在しないIDも `default_id` に置き換わります。

```console
curl -X POST http://127.0.0.1:8080/api/v1-k7m4q2/tts \
  -H "Content-Type: application/json" \
  -d '{"id":"258599616","text":"こんにちは"}'
```

```json
{
  "license": "Aivis Common Model License (ACML) 1.0",
  "url": "https://tts.markn2000.com/audio/7f4a....ogg"
}
```

返されたURLへGETすると、`audio/ogg` の音声を取得できます。

## Cloudflare Tunnel

Cloudflare Tunnel は `tts.markn2000.com` を `http://127.0.0.1:8080` へ転送するよう設定します。生成 API のレート制限は Cloudflare 側で `POST /api/*/tts` を対象に設定し、`GET /audio/*` は対象外とします。

APIの破壊的変更時は、推測しにくい新しい値へ `api_revision` を変更してください。これは認証情報ではありません。

詳しい動作は [SPEC.md](SPEC.md) を参照してください。

## 開発時の確認

```console
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```
