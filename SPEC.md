# TTS 仲介サーバー仕様

## 1. 概要

VOICEVOX Engine または AivisSpeech Engine にテキストを渡して WAV を生成し、FFmpeg で Ogg に変換して期限付きで公開する。

同じ話者 ID と同じ UTF-8 テキストによるリクエストでは、保存済みの Ogg を再利用する。

仲介サーバーは Windows / Linux 向けに、それぞれ単一の Rust 実行ファイルとして配布する。TTS Engine、FFmpeg、Cloudflare Tunnel は外部プロセスとする。

## 2. API

### 音声生成

```http
POST /api/{api_revision}/tts
Content-Type: application/json
```

例：

```http
POST /api/v1/tts
```

```json
{
  "id": "258599616",
  "text": "こんにちは"
}
```

- `text` は必須とする。
- `id` は文字列で、省略可能とする。
- `api_revision` は設定ファイルの値と完全一致するパスだけを有効とし、それ以外は `404 Not Found` とする。
- `id` が省略されている場合、または Engine から取得した話者一覧に存在しない場合は `default_id` を使用する。
- `default_id` が Engine から取得した話者一覧に存在しない場合は起動エラーとする。

成功時は、実際に使用した話者のライセンスと音声 URL だけを返す。

```json
{
  "license": "Aivis Common Model License (ACML) 1.0",
  "url": "https://tts.markn2000.com/audio/7f4a....ogg"
}
```

### 音声取得

```http
GET /audio/{audio_id}.ogg
```

- `Content-Type: audio/ogg` で保存済み音声を返す。
- 音声生成用の GET API は提供しない。

### 話者一覧

```http
GET /speakers
```

- 起動時に TTS Engine の `/speakers` から取得した JSON を加工せずに返す。
- `Content-Type: application/json` とする。
- 起動中は同じ内容を返し、更新には仲介サーバーの再起動を必要とする。

### LAN 内管理画面

音声生成 Web UI と設定画面・管理 API は、公開 API と別の `admin_listen` で提供する。

```http
GET /webui
GET /settings
```

- 画面の HTML、CSS、JavaScript は実行ファイルへ埋め込み、追加ファイルとして配布しない。
- `admin_listen` にはプライベート IPv4、ループバックアドレス、または IPv6 のユニークローカルアドレスだけを指定できる。
- Cloudflare Tunnel は公開用の `listen` だけへ接続し、`admin_listen` は公開しない。
- 公開用の `listen` では、設定画面、Web UI 用の音声生成 API、次の管理 API を提供しない。
- LAN 外からの直接接続は OS のファイアウォールでも拒否する。

#### 設定画面

`GET /settings` では、リンク、ユーザー辞書、アップデートを同じ画面に表示する。旧 `GET /dictionary` は提供しない。

```http
GET /api/settings
```

実行中の設定から、画面へ表示するリンクを返す。

```json
{
  "public_tts_url": "https://tts.markn2000.com/api/v1/tts"
}
```

- 公開 TTS API URL は `public_base_url`、`api_revision` から組み立てる。
- 設定画面では公開 TTS API URL を表示し、同一オリジンの音声生成 Web UI と話者一覧 API をリンクとして開けるようにする。

#### 音声生成 Web UI

`GET /webui` では、起動時に取得した話者・スタイルを選び、入力したテキストから音声を生成する画面を提供する。

- 話者一覧には管理用の `GET /speakers` を使用し、公開用と同じ未加工の JSON を返す。
- 画面では話者名とスタイル名を表示し、音声生成時の `id` にはスタイル ID を使用する。
- 既定の話者・スタイルが選ばれた場合は `id` を省略し、`default_id` を使用する。
- 入力テキストのトリム、Unicode 正規化、その他の自動変換は行わない。

```http
POST /api/webui/tts
Content-Type: application/json
```

リクエスト本文は公開用の音声生成 API と同じ形式とする。成功時は、実際に使用した話者のライセンスと、管理画面と同一オリジンの相対音声 URL を返す。

```json
{
  "license": "Aivis Common Model License (ACML) 1.0",
  "url": "/audio/7f4a....ogg"
}
```

- 音声生成、キャッシュ、排他制御は公開用の音声生成 API と共通にする。
- 応答は Ogg の生成と保存が完了してから返し、生成中の WAV は配信しない。
- 画面では返された Ogg を再生し、同じ URL からファイルとしてダウンロードできるようにする。
- 管理用の `GET /audio/{audio_id}.ogg` は、公開用と同じ保存済み Ogg を `Content-Type: audio/ogg` で返す。
- Web UI の HTML、CSS、JavaScript は実行ファイルへ埋め込み、追加ファイルとして配布しない。

#### ユーザー辞書取得

```http
GET /api/user-dict
```

```json
{
  "words": [
    {
      "uuid": "00000000-0000-0000-0000-000000000000",
      "surface": "単語",
      "pronunciation": "タンゴ",
      "accent_type": 1,
      "word_type": "PROPER_NOUN",
      "priority": 5
    }
  ],
  "has_excluded_words": false
}
```

#### ユーザー辞書追加

```http
POST /api/user-dict/words
Content-Type: application/json
```

```json
{
  "surface": "単語",
  "pronunciation": "タンゴ",
  "accent_type": 1,
  "word_type": "PROPER_NOUN",
  "priority": 5
}
```

成功時は `201 Created` と追加された単語の UUID を返す。

```json
{
  "uuid": "00000000-0000-0000-0000-000000000000"
}
```

#### ユーザー辞書試聴

```http
POST /api/user-dict/preview
Content-Type: application/json
```

```json
{
  "pronunciation": "タンゴ",
  "accent_type": 1
}
```

- 入力中の読みとアクセント位置を使用し、`default_id` の話者で試聴用 WAV を生成する。
- 成功時は `Content-Type: audio/wav`、`Cache-Control: no-store` で WAV を直接返す。
- 試聴ではユーザー辞書を変更せず、音声ファイルとキャッシュを保存しない。
- 入力したカタカナから生成した音声クエリのモーラを1つのアクセント句へまとめ、指定したアクセント位置で音高を再計算して合成する。
- 読みとモーラの照合時だけ、AivisSpeech が正規化する `ヂ→ジ`、`ヅ→ズ`、`ヰ→イ`、`ヱ→エ`、`ヲ→オ` を同じものとして扱う。画面の入力値とユーザー辞書へ保存する読みは変更しない。
- アクセント位置がモーラ数を超える場合は `400 Bad Request` とする。
- 試聴は音声生成および辞書変更と同じ排他制御内で実行する。

#### ユーザー辞書更新・削除

```http
PUT /api/user-dict/words/{word_uuid}
Content-Type: application/json
```

PUT の本文は追加時と同じ形式とし、成功時は `204 No Content` を返す。

```http
DELETE /api/user-dict/words/{word_uuid}
```

削除成功時は `204 No Content` を返す。

- 編集対象は VOICEVOX と AivisSpeech に共通する単一語だけとする。
- `word_type` は `PROPER_NOUN`、`COMMON_NOUN`、`VERB`、`ADJECTIVE`、`SUFFIX` の5種類とする。
- AivisSpeech 固有の複合語および共通外の品詞は一覧と編集対象から除外し、`has_excluded_words` で存在を通知する。
- 辞書は TTS Engine 側を正とし、仲介サーバーには複製しない。
- 管理画面を開いたときと辞書変更後に一覧を取得し、画面上の「辞書を再読み込み」からも最新一覧を取得できる。
- 一覧の再読み込みだけでは音声キャッシュを削除しない。
- 読みへ入力したひらがなは入力確定後にカタカナへ変換する。
- アクセント位置は、読みをモーラ単位で表示した高低図とスライダーで選択する。スライダー右端は平板を表す。
- 優先度は 0 から 10 までのスライダーで選択し、現在値を併記する。
- 追加、更新、削除は音声生成と同じ排他制御内で行い、辞書変更前に音声キャッシュを全削除する。
- 音声キャッシュの削除に失敗した場合は、TTS Engine の辞書を変更しない。
- エラー応答は `{ "error": "メッセージ" }` とする。

#### Linux 自己更新

設定画面では、Linux x86_64 で動作している場合に限り、GitHub の最新リリースを確認して実行ファイルを更新できる。

```http
GET /api/update
```

```json
{
  "supported": true,
  "current_version": "0.2.0",
  "latest_version": "0.3.0",
  "update_available": true
}
```

- 最新版の確認には `MarkN2000/tts` の GitHub Releases にある最新の正式リリースを使用する。
- タグは `v` を除いた SemVer として解釈し、現在の実行ファイルより新しい場合だけ更新可能とする。
- Windows および Linux x86_64 以外では `supported` を `false` とし、更新を実行できない。

```http
GET /api/version
```

- GitHubへ接続せず、実行中のバージョンと自己更新対応の有無を返す。更新後の再接続確認に使用する。

```http
POST /api/update
```

- 更新対象は、固定名 `tts-server-linux-x86_64` で公開された raw 実行ファイルだけとする。
- アーカイブ、任意の URL、任意のバージョンは更新対象にしない。
- ダウンロードしたファイルは現在の実行ファイルと同じディレクトリへ一時保存し、リリース情報のサイズと SHA-256 digest、ELF x86_64 形式、埋め込まれたバージョンを検証する。
- 現行実行ファイルは同じディレクトリへ、現在のファイル名に `.previous` を加えた名前（通常は `tts-server.previous`）で1世代だけ保存する。`config.toml` とキャッシュは変更しない。
- 検証後のファイルを現在の実行ファイルのパスへ原子的に置換し、HTTP 応答を返した後にサーバーを正常終了する。
- 成功時は `202 Accepted` と、再起動後のバージョンを返す。画面はサーバーへ再接続し、実行中のバージョンを確認して完了を表示する。
- 更新には、サービス実行ユーザーが実行ファイルのあるディレクトリへ書き込めることと、systemd の `Restart=always` が必要となる。
- ダウンロード、検証、保存、置換に失敗した場合はサーバーを終了せず、現在の実行ファイルを使用し続ける。
- 新しい実行ファイルが起動できない場合の自動復旧は行わない。保存した `tts-server.previous` を管理者が戻す。
- 更新確認と更新実行は辞書管理 API と同じく `admin_listen` だけで提供する。

## 3. 音声生成

- 接続する TTS Engine は設定ファイルで1つだけ指定し、リクエストでは切り替えない。
- TTS Engine の HTTP API を利用して WAV を生成する。
- WAV は FFmpeg で Ogg Opus または Ogg Vorbis に変換する。
- コーデックとビットレートは設定可能とする。
- 変換後の Ogg だけを保存し、作業用 WAV と不完全な出力は残さない。
- 音声生成の同時実行数は全体で1件とする。キャッシュ済み音声の取得は並行して処理できる。
- 公開 API と音声生成 Web UI の未生成音声リクエストは、生成中の1件を含めて最大10件まで受け付け、残りを到着順に待機させる。辞書の試聴はこの受付上限に含めない。
- 受付上限を超えた場合は `503 Service Unavailable` と `Retry-After: 10` を返す。
- キャッシュ済み音声を返すリクエストは受付上限に含めない。

## 4. 話者

- 起動時に TTS Engine の `/speakers` から利用可能な話者 ID、スタイル、`speaker_uuid` を取得する。
- `/speakers` の取得結果は生の JSON としてもメモリに保持し、公開用の話者一覧に使用する。
- 各 `speaker_uuid` について `/speaker_info` を取得し、`policy` の先頭にある空でない Markdown 見出しから先頭の `#` を除いたライセンス名を自動取得する。
- ライセンス名を取得できない場合は `Unknown` とする。
- 未登録 ID はエラーにせず、`default_id` に置き換える。
- 応答の `license` には、置き換え後に実際に使用した話者から取得したライセンス名を使用する。
- 話者情報とライセンスは起動時に取得してメモリに保持し、再取得には仲介サーバーの再起動を必要とする。

## 5. キャッシュ

- キャッシュの一致条件は、置き換え後の話者 ID とデコード済み UTF-8 テキストだけとする。
- テキストのトリム、Unicode 正規化、その他の自動変換は行わない。
- 公開用 `audio_id` は一致条件から決定的に生成し、方式は外部 API の仕様に含めない。
- 保存期間は `cache_days` で日数指定する。
- 保存期間は生成時点から数え、アクセスされても延長しない。
- 期限切れファイルはキャッシュミスとして再生成し、起動時および定期的に削除する。
- キャッシュ容量の上限は `cache_max_mb` で指定し、配布する `config.toml` の初期値は `1024`（1 GiB相当）とする。
- 期限切れファイルを削除しても容量上限を超えている場合は、生成日時が古い Ogg から削除する。
- 容量上限によって、`cache_days` より前に音声が削除されることがある。
- 新しく生成した音声は、その生成直後に行う容量整理では削除しない。
- 起動中にキャッシュディレクトリが削除された場合は、次の音声生成または定期削除時にディレクトリとキャッシュ状態を自動で再作成する。
- 音声生成中にキャッシュディレクトリが削除された場合、その生成は失敗することがあるが、次の音声生成時に復旧する。
- 同じキャッシュに対する生成中のリクエストは、先行する1件の完了を待って同じ結果を使用する。

## 6. 設定の反映とキャッシュ削除

- `config.toml` は起動時に1回だけ読み込む。
- `config.toml` は実行ファイルと同じディレクトリから読み込む。
- 相対指定の `cache_dir` は、`config.toml` があるディレクトリを基準に解決する。
- 起動中のファイル変更は反映せず、反映には再起動を必要とする。
- `api_revision` は `v1`、`v2` のように公開 API のリビジョンを表す値とする。
- API の破壊的変更時は `api_revision` を変更し、古い API パスは提供しない。
- `api_revision` は古い仕様の利用とバージョン番号だけによる誤接続を防ぐための値であり、認証情報としては扱わない。
- 起動時に、音声内容へ影響する次の設定を前回起動時の値と比較する。
  - TTS Engine の接続先
  - `cache_revision`
  - コーデック
  - ビットレート
- 比較対象が変更されていた場合は、既存キャッシュをすべて削除する。
- 同じ接続先のまま Engine または音声モデルを変更した場合は、利用者が `cache_revision` を変更する。
- `api_revision`、`cache_days`、`cache_max_mb`、公開 URL、`default_id` の変更だけではキャッシュを全削除しない。
- `admin_listen` の変更ではキャッシュを削除しない。

## 7. 設定ファイル例

```toml
listen = "127.0.0.1:8080"
admin_listen = "192.168.1.10:8081"
engine_url = "http://192.168.1.11:10101"
public_base_url = "https://tts.markn2000.com"
api_revision = "v1"

default_id = "1878365376"

cache_dir = "./cache"
cache_days = 7
cache_max_mb = 1024
cache_revision = 1

ffmpeg_path = "ffmpeg"
codec = "vorbis"
bitrate_kbps = 48
```

## 8. 配布と実行条件

配布物は OS ごとの実行ファイルと `config.toml` の2ファイルとする。

```text
tts-server.exe
config.toml
```

- FFmpeg は同梱せず、`ffmpeg_path` または `PATH` から実行できることを前提とする。
- 指定した TTS Engine は別途起動済みであることを前提とする。
- キャッシュディレクトリは初回起動時に作成する。
- FFmpeg、設定、TTS Engine への接続と話者情報の取得を起動時に確認し、利用できない場合は起動エラーとする。
- GitHub Release には、通常の Linux x86_64 用 tar.gz に加えて、自己更新用の固定名 `tts-server-linux-x86_64` を添付する。

## 9. アクセス制限

- 生成 API は認証なしで公開する。
- `POST /api/*/tts` へのレート制限は Cloudflare 側で行う。
- 音声取得用の `GET /audio/{audio_id}.ogg` は生成 API のレート制限対象に含めない。
- URL の `api_revision` はアクセス制限の代替として扱わない。
- 音声生成 Web UI、Web UI 用の音声生成 API、設定画面、管理 API は `admin_listen` だけで提供し、Cloudflare Tunnel の接続先に含めない。
- LAN 内の利用者は音声を生成し、辞書を変更し、Linux ではサーバーを最新版へ更新して再起動できるため、信頼できるネットワークでだけ使用する。

## 10. 初版の対象外

- リクエストごとの VOICEVOX / AivisSpeech 切り替え
- アプリ内の認証、API キー、利用者別レート制限
- 音声生成用 GET API
- 起動中の設定ファイル再読み込み
- FFmpeg の同梱
- ユーザー辞書の検索、一括操作、インポート、エクスポート
- AivisSpeech 固有の複合語および共通外の品詞の編集
