const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const script = fs.readFileSync(path.join(__dirname, "../web/dictionary.js"), "utf8");
const html = fs.readFileSync(path.join(__dirname, "../web/dictionary.html"), "utf8");
const source = script.split("\nconst elements = {")[0];
const context = vm.createContext({});
vm.runInContext(
  `${source}\nthis.toKatakana = hiraganaToKatakana; this.splitMoras = splitPronunciationMoras; this.pitchLevels = accentPitchLevels; this.toSlider = accentTypeToSliderValue; this.toAccent = sliderValueToAccentType; this.valueText = accentValueText;`,
  context,
);

test("ひらがなをカタカナへ変換する", () => {
  assert.equal(context.toKatakana("かいづか"), "カイヅカ");
  assert.equal(context.toKatakana("ゔぉーかる"), "ヴォーカル");
  assert.equal(context.toKatakana("カタカナ・漢字123"), "カタカナ・漢字123");
});

test("小書きカタカナを直前の文字と同じモーラへまとめる", () => {
  const cases = [
    ["カイヅカ", ["カ", "イ", "ヅ", "カ"]],
    ["キャラクター", ["キャ", "ラ", "ク", "タ", "ー"]],
    ["ティッシュ", ["ティ", "ッ", "シュ"]],
    ["ヴォーカル", ["ヴォ", "ー", "カ", "ル"]],
  ];

  for (const [pronunciation, expected] of cases) {
    assert.deepEqual(Array.from(context.splitMoras(pronunciation)), expected);
  }
});

test("平板と各アクセント位置の簡易高低を求める", () => {
  assert.deepEqual(Array.from(context.pitchLevels(4, 0)), [0, 1, 1, 1, 1]);
  assert.deepEqual(Array.from(context.pitchLevels(4, 1)), [1, 0, 0, 0, 0]);
  assert.deepEqual(Array.from(context.pitchLevels(4, 2)), [0, 1, 0, 0, 0]);
  assert.deepEqual(Array.from(context.pitchLevels(4, 4)), [0, 1, 1, 1, 0]);
});

test("スライダー右端だけを平板へ変換する", () => {
  assert.equal(context.toSlider(4, 1), 1);
  assert.equal(context.toSlider(4, 0), 5);
  assert.equal(context.toAccent(4, 4), 4);
  assert.equal(context.toAccent(4, 5), 0);
});

test("アクセントのスライダー値を意味のある文言で表す", () => {
  assert.equal(context.valueText(0), "平板");
  assert.equal(context.valueText(3), "アクセント 3");
});

test("設定画面が公開API URLを取得して表示する", () => {
  assert.match(html, /id="public-tts-url"/u);
  assert.match(html, /href="\/webui"/u);
  assert.match(html, /href="\/speakers"/u);
  assert.match(script, /fetch\("\/api\/settings"/u);
  assert.match(script, /settings\.public_tts_url/u);
});
