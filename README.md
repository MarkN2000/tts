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

`config.toml` の `[[engines]]`、`public_base_url` などを環境に合わせて変更し、実行ファイルを起動してください。各 Engine にはURLで使用する一意な `id`、表示用の `name`、TTS本体の `engine_url`、`default_id` を指定します。相対指定の `cache_dir` は、このディレクトリを基準にします。LAN内管理画面用の `admin_listen` には、管理画面を開く端末から接続できる、このPCのプライベートIPアドレスを指定します。

配布する `config.toml` はAivisSpeech 1件だけを有効にし、VOICEVOXの追加例をコメントアウトで記載しています。VOICEVOXを起動して該当行のコメントを外すと、2台目として追加できます。設定したEngineはすべて起動時に接続確認され、1件でも利用できない場合はサーバーを起動しません。複数登録の例は [SPEC.md](SPEC.md#7-設定ファイル例) を参照してください。

### 旧設定からの移行

トップレベルに `engine_url` と `default_id` がある旧形式の `config.toml` は、新しい実行ファイルの初回起動時に自動移行します。既存Engineは `aivisspeech` として登録され、`api_revision` はそのまま維持されます。旧設定でVOICEVOXを使用していた場合は、移行後に `id` と `name` を変更してください。

移行前の設定は `config.toml.pre-engines` にそのまま保存されます。旧形式と `[[engines]]` が混在している場合や、既存バックアップの内容が現在の旧設定と異なる場合は、設定を変更せず起動を中止します。

この変更では公開APIと音声URLのパスが変わり、更新前に発行した音声URLは利用できなくなります。Engineを指定しない固定のv1互換URLとして `/api/v1/tts` と `/api/v1/speakers` を利用できますが、それ以外は設定画面に表示される新しいURLへ呼び出し元を変更してください。

## API

音声を生成します。`speaker` は省略でき、存在しないスタイルIDも `default_id` に置き換わります。`text` と `speaker` はURLのクエリパラメータとしてエンコードしてください。

GETとPOSTのどちらでも同じ結果を返します。

```console
curl "http://127.0.0.1:8080/api/v2/aivisspeech/tts?text=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF&speaker=1878365376"
curl -X POST "http://127.0.0.1:8080/api/v2/aivisspeech/tts?text=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF&speaker=1878365376"
```

Engineを指定しない `/api/v1/tts` は互換URLとして、設定順の先頭Engineで生成します。リクエスト形式は上記と同じクエリ形式で、返却される音声URLはEngine IDを含む新形式です。旧音声URLと古いJSON形式には対応しません。

公開APIでは末尾の `/` があっても同じパスとして扱います。正規URLは末尾 `/` なしです。

```text
https://tts.example.com/audio/aivisspeech/7f4a....ogg?license=Aivis+Common+Model+License+%28ACML%29+1.0
```

返されたURLへGETすると、`audio/ogg` の音声を取得できます。

話者ID・名前・スタイルは、起動時にEngineから取得した内容を返す次のAPIで確認できます。

```console
curl http://127.0.0.1:8080/api/v2/aivisspeech/speakers
```

Engineを指定しない `GET /api/v1/speakers` は、設定順の先頭Engineの話者一覧を返します。旧 `GET /speakers` は提供しません。

## 音声生成 Web UI

起動後、サーバーと同じPCのブラウザで次の画面を開くと、Engine、話者・スタイルとテキストを選んで音声を生成できます。`?engine=aivisspeech` のようにEngine IDを指定して開くこともできます。生成後は保存済みの Ogg を再生し、そのままファイルとしてダウンロードできます。

```text
http://127.0.0.1:8081/webui
```

LAN内の別端末から使用する場合は、`admin_listen` をサーバーのプライベートIPアドレスへ変更し、URLもその値に合わせてください。Web UI は公開用ポートや Cloudflare Tunnel では提供しません。

## 設定画面

起動後、サーバーと同じPCのブラウザで次の設定画面を開くと、音声生成Web UI・公開API・話者一覧へのリンクを確認できます。同じ画面で、音声キャッシュの使用状況確認と削除、VOICEVOXとAivisSpeechに共通するユーザー辞書の編集、Linux版の再起動、Linux x86_64版のアップデートも行えます。画面は実行ファイルへ埋め込まれているため、配布ファイルは増えません。

```text
http://127.0.0.1:8081/settings
```

URLのIPアドレスとポートは `admin_listen` に合わせてください。辞書を変更すると、対象Engineで変更前の辞書から生成された音声キャッシュはすべて削除されます。

設定画面には認証がなく、LAN内の利用者は辞書を変更できます。`admin_listen` は信頼できるLANでだけ使用し、OSのファイアウォールでは必要なLANサブネットからの接続だけを許可してください。

## Linuxでの再起動とアップデート

Linuxでは、`config.toml` を変更した後に設定画面の「設定を反映して再起動」から反映できます。再起動前に設定ファイル、FFmpeg、各TTS Engineの話者一覧と既定話者を検証し、不正な場合は現在のサーバーを終了しません。キャッシュディレクトリと変更後の待受ポートは再起動後に確認されるため、それらに問題がある場合は起動に失敗します。`admin_listen` を変更した場合は、変更後のアドレスで設定画面を開き直してください。

Linux x86_64 では、設定画面の「アップデートを確認」から最新リリースへ更新できます。更新すると実行ファイルだけを差し替えてサーバーが正常終了し、systemdによって再起動されます。アップデータは `config.toml` と音声キャッシュを変更しませんが、新しい実行ファイルの初回起動時に旧形式の設定とキャッシュを前述の仕様で移行します。

systemdのサービスには次の設定が必要です。

```ini
[Service]
Restart=always
RestartSec=2s
```

サービスを実行するユーザーには、`tts-server`を置いたディレクトリへの書き込み権限を与えてください。更新前の実行ファイルは同じディレクトリの`tts-server.previous`へ1世代だけ保存されます。新しい実行ファイルが起動できない場合はサービスを停止し、このファイルを`tts-server`へ戻してください。設定が新形式へ移行済みなら、`config.toml.pre-engines` も `config.toml` へ戻してから起動してください。

再起動とアップデート操作にも認証はありません。信頼できるLAN内の利用者だけが設定画面へ接続できるようにしてください。

## Cloudflare Tunnel

Cloudflare Tunnelを使用する場合は、設定した公開ホスト名を公開用の `http://127.0.0.1:8080` だけへ転送します。管理用の8081番ポートはTunnelへ設定しないでください。Cloudflareのレート制限は URI Path を `wildcard` `/api/*` として公開API全体へ適用できます。この指定には旧・新の音声生成APIと話者一覧が含まれ、`/audio/*` は含まれません。

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
