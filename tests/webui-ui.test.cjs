const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const html = fs.readFileSync(path.join(__dirname, "../web/webui.html"), "utf8");
const styles = fs.readFileSync(path.join(__dirname, "../web/webui.css"), "utf8");
const source = fs.readFileSync(path.join(__dirname, "../web/webui.js"), "utf8")
  .split("\nconst elements = {")[0];
const context = vm.createContext({});
vm.runInContext(
  `${source}\nthis.makeFileName = makeDownloadFileName; this.playAudio = playGeneratedAudio; this.resizeTextArea = resizeTextArea;`,
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
