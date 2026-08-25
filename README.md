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

`config.toml` の `engine_url`、`public_base_url`、`default_id` などを環境に合わせて変更し、実行ファイルを起動してください。相対指定の `cache_dir` は、このディレクトリを基準にします。LAN内管理画面用の `admin_listen` には、管理画面を開く端末から接続できる、このPCのプライベートIPアドレスを指定します。

## API

音声を生成します。`speaker` は省略でき、存在しないスタイルIDも `default_id` に置き換わります。`text` と `speaker` はURLのクエリパラメータとしてエンコードしてください。

GETとPOSTのどちらでも同じ結果を返します。

```console
curl "http://127.0.0.1:8080/api/v2/tts?text=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF&speaker=1878365376"
curl -X POST "http://127.0.0.1:8080/api/v2/tts?text=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF&speaker=1878365376"
```

```text
https://tts.example.com/audio/7f4a....ogg?license=Aivis+Common+Model+License+%28ACML%29+1.0
```

返されたURLへGETすると、`audio/ogg` の音声を取得できます。

話者ID・名前・スタイルは、起動時にEngineから取得した内容を返す次のAPIで確認できます。

```console
curl http://127.0.0.1:8080/speakers
```

## 音声生成 Web UI

起動後、サーバーと同じPCのブラウザで次の画面を開くと、話者・スタイルとテキストを選んで音声を生成できます。生成後は保存済みの Ogg を再生し、そのままファイルとしてダウンロードできます。

```text
http://127.0.0.1:8081/webui
```

LAN内の別端末から使用する場合は、`admin_listen` をサーバーのプライベートIPアドレスへ変更し、URLもその値に合わせてください。Web UI は公開用ポートや Cloudflare Tunnel では提供しません。

## 設定画面

起動後、サーバーと同じPCのブラウザで次の設定画面を開くと、音声生成Web UI・公開API・話者一覧へのリンクを確認できます。同じ画面で、音声キャッシュの使用状況確認と削除、VOICEVOXとAivisSpeechに共通するユーザー辞書の編集、Linux x86_64版のアップデートも行えます。画面は実行ファイルへ埋め込まれているため、配布ファイルは増えません。

```text
http://127.0.0.1:8081/settings
```

URLのIPアドレスとポートは `admin_listen` に合わせてください。辞書を変更すると、変更前の辞書で生成された音声キャッシュはすべて削除されます。

設定画面には認証がなく、LAN内の利用者は辞書を変更できます。`admin_listen` は信頼できるLANでだけ使用し、OSのファイアウォールでは必要なLANサブネットからの接続だけを許可してください。

## Linuxでのアップデート

Linux x86_64 では、設定画面の「アップデートを確認」から最新リリースへ更新できます。更新すると実行ファイルだけを差し替えてサーバーが正常終了し、systemdによって再起動されます。`config.toml`と音声キャッシュは変更しません。

systemdのサービスには次の設定が必要です。

```ini
[Service]
Restart=always
RestartSec=2s
```

サービスを実行するユーザーには、`tts-server`を置いたディレクトリへの書き込み権限を与えてください。更新前の実行ファイルは同じディレクトリの`tts-server.previous`へ1世代だけ保存されます。新しい実行ファイルが起動できない場合は、サービスを停止し、このファイルを`tts-server`へ戻してから起動してください。

アップデート操作にも認証はありません。信頼できるLAN内の利用者だけが設定画面へ接続できるようにしてください。

## Cloudflare Tunnel

Cloudflare Tunnelを使用する場合は、設定した公開ホスト名を公開用の `http://127.0.0.1:8080` だけへ転送します。管理用の8081番ポートはTunnelへ設定しないでください。生成 API のレート制限は Cloudflare 側で `GET /api/*/tts` と `POST /api/*/tts` を対象に設定し、`GET /audio/*` は対象外とします。

すべてのHTTP応答には `X-Robots-Tag: noindex` を付与し、検索エンジンのインデックス登録を抑制します。これはアクセス制限ではないため、クローラーを含むリクエスト自体は拒否しません。

APIの破壊的変更時は、新しい値へ `api_revision` を変更してください。これは認証情報ではありません。

詳しい動作は [SPEC.md](SPEC.md) を参照してください。

## 開発時の確認

```console
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

## リリース

`v` で始まるタグをGitHubへpushすると、GitHub Actionsが次の配布物を自動作成し、GitHub Releaseへ添付します。

- Windows x86_64: ZIP
- Linux x86_64: musl静的リンクのtar.gzと、自己更新用raw実行ファイル

WindowsのZIPとLinuxのtar.gzには、実行ファイルと`config.toml`が含まれます。
