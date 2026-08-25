const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const html = fs.readFileSync(path.join(__dirname, "../web/webui.html"), "utf8");
const styles = fs.readFileSync(path.join(__dirname, "../web/webui.css"), "utf8");
const source = fs.readFileSync(path.join(__dirname, "../web/webui.js"), "utf8")
  .split("\nconst elements = {")[0];
const context = vm.createContext({ URL, URLSearchParams });
vm.runInContext(
  `${source}\nthis.makeFileName = makeDownloadFileName; this.playAudio = playGeneratedAudio; this.resizeTextArea = resizeTextArea; this.makeRequestUrl = makeTtsRequestUrl; this.parseResponse = parseTtsResponse;`,
  context,
);

test("テキスト入力欄は1行から始まり、手動リサイズとスクロールバーを表示しない", () => {
  assert.match(html, /<textarea[^>]+rows="1"/u);
  assert.match(
    styles,
    /textarea \{ min-height: 44px; overflow-y: hidden; resize: none; \}/u,
  );
});

test("テキスト入力欄を内容と境界線に合う高さへ調整する", () => {
  const textarea = {
    style: { height: "200px" },
    offsetHeight: 44,
    clientHeight: 42,
    get scrollHeight() {
      assert.equal(this.style.height, "auto");
      return 80;
    },
  };

  context.resizeTextArea(textarea);

  assert.equal(textarea.style.height, "82px");
});

test("画面上部の見出しを置かず、状態を生成ボタンの左へ表示する", () => {
  assert.doesNotMatch(html, /TTS SERVER|<h[1-6]|class="page-header"/u);
  assert.doesNotMatch(html, /id="form-title"/u);
  assert.doesNotMatch(html, /id="result-title"|aria-labelledby="result-title"/u);
  assert.match(html, /id="result"[^>]+aria-label="生成した音声"/u);
  assert.match(
    html,
    /class="form-actions">\s*<a class="settings-link" href="\/settings">設定<\/a>\s*<p id="status"[^>]*>[^<]*<\/p>\s*<button id="generate-button"/u,
  );
  assert.match(styles, /\.form-actions \{[\s\S]*align-items: center;/u);
  assert.match(styles, /\.settings-link \{[\s\S]*margin-right: auto;[\s\S]*font-size: \.82rem;/u);
  assert.match(styles, /@media \(max-width: 640px\) \{[\s\S]*flex-direction: column;/u);
  assert.match(styles, /\.form-actions \.settings-link \{[^}]*margin-right: 0;/u);
});

test("話者名とスタイル名とテキストからOGGファイル名を作る", () => {
  assert.equal(
    context.makeFileName("ずんだもん", "ノーマル", "こんにちは、世界！"),
    "ずんだもん_ノーマル_こんにちは、世界！.ogg",
  );
});

test("ファイル名に使えない文字と改行を空白へ置き換える", () => {
  assert.equal(
    context.makeFileName("モデル/A", "感情:強", "1行目\n2行目?*"),
    "モデル A_感情 強_1行目 2行目.ogg",
  );
});

test("各項目を指定された長さへ切り詰める", () => {
  const fileName = context.makeFileName("話".repeat(31), "型".repeat(31), "文".repeat(21));
  assert.equal(fileName, `${"話".repeat(30)}_${"型".repeat(30)}_${"文".repeat(20)}….ogg`);
});

test("テキストが20文字以内なら省略記号を付けない", () => {
  assert.equal(
    context.makeFileName("モデル", "通常", "文".repeat(20)),
    `モデル_通常_${"文".repeat(20)}.ogg`,
  );
});

test("空の名前とテキストには既定名を使う", () => {
  assert.equal(context.makeFileName("", "", ""), "既定モデル_音声.ogg");
});

test("生成した音声を自動再生し、再生拒否は生成エラーにしない", () => {
  let playCount = 0;
  let rejectionHandler;
  const audio = {
    play() {
      playCount += 1;
      return {
        catch(handler) {
          rejectionHandler = handler;
        },
      };
    },
  };

  context.playAudio(audio);

  assert.equal(playCount, 1);
  assert.equal(typeof rejectionHandler, "function");
  assert.doesNotThrow(() => rejectionHandler(new Error("自動再生が拒否されました")));
});

test("Web UIの音声生成もtextとspeakerをクエリで送る", () => {
  assert.equal(
    context.makeRequestUrl("こんにちは & おはよう", "258599616"),
    "/api/webui/tts?text=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF+%26+%E3%81%8A%E3%81%AF%E3%82%88%E3%81%86&speaker=258599616",
  );
  assert.equal(
    context.makeRequestUrl("こんにちは", ""),
    "/api/webui/tts?text=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF",
  );
});

test("プレーンテキストの音声URLからライセンスを取得する", () => {
  const result = context.parseResponse(
    "/audio/example.ogg?license=Aivis+Common+Model+License+%28ACML%29+1.0",
  );
  assert.equal(result.url, "/audio/example.ogg?license=Aivis+Common+Model+License+%28ACML%29+1.0");
  assert.equal(result.license, "Aivis Common Model License (ACML) 1.0");
});

test("ライセンスのない音声URLは不正な応答として拒否する", () => {
  assert.throws(
    () => context.parseResponse("/audio/example.ogg"),
    /音声生成の応答が不正です/u,
  );
});
