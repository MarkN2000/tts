# TTS Server

複数の VOICEVOX Engine / AivisSpeech Engine の音声合成 API を仲介し、生成した WAV を FFmpeg で Oggへ変換して公開する小さなサーバーです。

## 必要なもの

- 1つ以上の起動済み VOICEVOX Engine または AivisSpeech Engine
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

`config.toml` を環境に合わせて編集し、実行ファイルを起動します。配布設定にはAivisSpeechと、コメントアウトしたVOICEVOXの追加例があります。Engineはすべて起動時に接続確認されます。

各 `[[engines]]` で `id`、`name`、`engine_url`、`default_id`、`attribution` を指定します。`admin_listen` をLANへ公開する場合は、サーバーのプライベートIPアドレスを使用してください。設定項目の詳細は [SPEC.md](SPEC.md#6-設定) を参照してください。

### 旧設定からの移行

トップレベルに `engine_url` と `default_id` がある旧形式は、初回起動時に新形式へ自動移行します。移行前の設定は `config.toml.pre-engines` へ保存されます。VOICEVOXを使用していた場合は、移行後にEngine設定を確認してください。

## API

音声生成はGETとPOSTに対応します。`speaker` は省略でき、未知のIDは `default_id` に置き換えます。

```console
curl "http://127.0.0.1:8080/api/v3/aivisspeech/tts?text=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF&speaker=1878365376"
```

Engineを省略した `/api/{api_revision}/tts` と `/api/{api_revision}/speakers` は、設定順の先頭Engineを使用します。

```text
https://tts.example.com/audio/aivisspeech/7f4a....ogg?license=Aivis+Common+Model+License+%28ACML%29+1.0
```

`license_from_policy` はライセンス名を `license`、`credit` は生成したクレジットを `credit` として音声URLへ含めます。既存設定で `attribution` を省略すると `license_from_policy` になります。`credit` を追加・変更する場合は `api_revision` も新しい値へ変更してください。

話者一覧は次のAPIで確認できます。

```console
curl http://127.0.0.1:8080/api/v3/aivisspeech/speakers
```

## 音声生成 Web UI

話者・スタイルとテキストを選び、Oggの生成・再生・ダウンロードができます。

```text
http://127.0.0.1:8081/webui
```

`?engine=aivisspeech` のようにEngine IDを指定できます。LAN内の別端末から使う場合は `admin_listen` に合わせてURLを変更してください。

## 設定画面

公開APIのURL確認、キャッシュ管理、ユーザー辞書編集、Linux版の再起動・アップデートを行えます。

```text
http://127.0.0.1:8081/settings
```

認証はありません。`admin_listen` は信頼できるLANでだけ使用してください。

## Linuxでの再起動とアップデート

Linuxでは設定画面から `config.toml` の検証と再起動ができます。Linux x86_64では実行ファイルのアップデートも行えます。再起動にはsystemdの次の設定が必要です。

```ini
[Service]
Restart=always
RestartSec=2s
```

実行ユーザーには配置ディレクトリへの書き込み権限が必要です。更新前の実行ファイルは `tts-server.previous` へ1世代保存されます。

## Cloudflare Tunnel

Tunnelには公開用の `http://127.0.0.1:8080` だけを転送し、管理用ポートは公開しないでください。レート制限は URI Path の `wildcard` `/api/*` で公開APIへ適用できます。

応答には `X-Robots-Tag: noindex` を付与しますが、アクセス制限にはなりません。

詳しい動作は [SPEC.md](SPEC.md) を参照してください。

## 開発時の確認

```console
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

## リリース

`Cargo.toml` と同じバージョンの `v` タグをGitHubへpushすると、GitHub Actionsが次の配布物を作成します。

- Windows x86_64: ZIP
- Linux x86_64: musl静的リンクのtar.gzと、自己更新用raw実行ファイル

WindowsのZIPとLinuxのtar.gzには、実行ファイルと`config.toml`が含まれます。
