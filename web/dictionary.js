const typeLabels = {
  PROPER_NOUN: "固有名詞",
  COMMON_NOUN: "普通名詞",
  VERB: "動詞",
  ADJECTIVE: "形容詞",
  SUFFIX: "接尾辞",
};

const combiningSmallKatakana = new Set(["ァ", "ィ", "ゥ", "ェ", "ォ", "ャ", "ュ", "ョ", "ヮ"]);
const katakanaPronunciationPattern = /^[ァ-ヴー]+$/u;
const svgNamespace = "http://www.w3.org/2000/svg";

function hiraganaToKatakana(value) {
  return value.replace(/[ぁ-ゖ]/gu, (character) => (
    String.fromCodePoint(character.codePointAt(0) + 0x60)
  ));
}

function splitPronunciationMoras(pronunciation) {
  const moras = [];
  for (const character of pronunciation) {
    if (combiningSmallKatakana.has(character) && moras.length > 0) {
      moras[moras.length - 1] += character;
    } else {
      moras.push(character);
    }
  }
  return moras;
}

function accentPitchLevels(moraCount, accentType) {
  if (!Number.isInteger(moraCount) || moraCount < 1
    || !Number.isInteger(accentType) || accentType < 0 || accentType > moraCount) return [];
  return Array.from({ length: moraCount + 1 }, (_, index) => {
    if (accentType === 1) return index === 0 ? 1 : 0;
    if (index === 0) return 0;
    if (accentType === 0) return 1;
    return index < accentType ? 1 : 0;
  });
}

function accentTypeToSliderValue(moraCount, accentType) {
  return accentType === 0 ? moraCount + 1 : accentType;
}

function sliderValueToAccentType(moraCount, sliderValue) {
  return sliderValue === moraCount + 1 ? 0 : sliderValue;
}

function accentValueText(accentType) {
  return accentType === 0 ? "平板" : `アクセント ${accentType}`;
}

const elements = {
  status: document.querySelector("#status"),
  error: document.querySelector("#error"),
  excluded: document.querySelector("#excluded-note"),
  form: document.querySelector("#word-form"),
  title: document.querySelector("#editor-title"),
  uuid: document.querySelector("#word-uuid"),
  surface: document.querySelector("#surface"),
  pronunciation: document.querySelector("#pronunciation"),
  wordType: document.querySelector("#word-type"),
  accentType: document.querySelector("#accent-type"),
  accentEmpty: document.querySelector("#accent-empty"),
  accentPicker: document.querySelector("#accent-picker"),
  accentSlider: document.querySelector("#accent-slider"),
  accentTrack: document.querySelector("#accent-track"),
  accentDiagram: document.querySelector("#accent-diagram"),
  accentLabels: document.querySelector("#accent-labels"),
  priority: document.querySelector("#priority"),
  priorityValue: document.querySelector("#priority-value"),
  list: document.querySelector("#word-list"),
  count: document.querySelector("#word-count"),
  empty: document.querySelector("#empty-message"),
  reload: document.querySelector("#reload-button"),
  add: document.querySelector("#add-button"),
  cancel: document.querySelector("#cancel-button"),
  preview: document.querySelector("#preview-button"),
  save: document.querySelector("#save-button"),
};

let words = [];
let busy = false;
let previewAudio;
let previewAudioUrl;
let previewAbortController;

elements.add.addEventListener("click", () => openEditor());
elements.reload.addEventListener("click", () => loadDictionary("最新の辞書を読み込みました。"));
elements.cancel.addEventListener("click", closeEditor);
elements.form.addEventListener("submit", saveWord);
elements.preview.addEventListener("click", previewWord);
elements.pronunciation.addEventListener("input", (event) => {
  if (!event.isComposing) {
    elements.pronunciation.value = hiraganaToKatakana(elements.pronunciation.value);
  }
  renderAccentPicker();
});
elements.accentSlider.addEventListener("input", updateAccentFromSlider);
elements.priority.addEventListener("input", updatePriorityLabel);

for (const field of elements.form.elements) {
  field.addEventListener("invalid", () => field.setAttribute("aria-invalid", "true"));
  field.addEventListener("input", () => field.setAttribute("aria-invalid", String(!field.validity.valid)));
}

loadDictionary();

function setMessage(message = "", isError = false) {
  elements.status.textContent = isError ? "" : message;
  elements.error.textContent = isError ? message : "";
}

function setBusy(value) {
  busy = value;
  elements.reload.disabled = busy;
  elements.add.disabled = busy;
  for (const button of elements.list.querySelectorAll("button")) button.disabled = busy;
  for (const field of elements.form.elements) field.disabled = busy;
}

async function loadDictionary(successMessage = "") {
  setBusy(true);
  elements.reload.textContent = "読み込み中…";
  setMessage("読み込み中です…");
  try {
    const response = await fetch("/api/user-dict", { cache: "no-store" });
    if (!response.ok) throw new Error(await readError(response));
    const dictionary = await response.json();
    words = dictionary.words;
    elements.excluded.hidden = !dictionary.has_excluded_words;
    renderWords();
    setMessage(successMessage);
  } catch (error) {
    setMessage(error.message || "ユーザー辞書を取得できませんでした。", true);
  } finally {
    elements.reload.textContent = "辞書を再読み込み";
    setBusy(false);
  }
}

function renderWords() {
  elements.list.replaceChildren();
  elements.count.textContent = `${words.length}件`;
  elements.empty.hidden = words.length !== 0;

  for (const word of words) {
    const card = document.createElement("article");
    card.className = "word-card";

    const content = document.createElement("div");
    content.className = "word-content";
    const surface = document.createElement("h3");
    surface.textContent = word.surface;
    const pronunciation = document.createElement("p");
    pronunciation.textContent = word.pronunciation;
    const details = document.createElement("p");
    details.className = "word-details";
    const accent = word.accent_type === 0 ? "平板" : `アクセント ${word.accent_type}`;
    details.textContent = `${typeLabels[word.word_type] || word.word_type} ・ ${accent} ・ 優先度 ${word.priority}`;
    content.append(surface, pronunciation, details);

    const actions = document.createElement("div");
    actions.className = "word-actions";
    const edit = document.createElement("button");
    edit.type = "button";
    edit.className = "secondary-button";
    edit.textContent = "編集";
    edit.addEventListener("click", () => openEditor(word));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "secondary-button danger-button";
    remove.textContent = "削除";
    remove.addEventListener("click", () => deleteWord(word));
    actions.append(edit, remove);
    card.append(content, actions);
    elements.list.append(card);
  }
  setBusy(busy);
}

function openEditor(word = null) {
  if (busy) return;
  releasePreview();
  elements.form.reset();
  elements.uuid.value = word?.uuid ?? "";
  elements.surface.value = word?.surface ?? "";
  elements.pronunciation.value = word?.pronunciation ?? "";
  elements.wordType.value = word?.word_type ?? "PROPER_NOUN";
  elements.accentType.value = String(word?.accent_type ?? 0);
  elements.priority.value = String(word?.priority ?? 5);
  elements.title.textContent = word ? "単語を編集" : "単語を追加";
  elements.save.textContent = word ? "変更を保存" : "辞書に追加";
  for (const field of elements.form.elements) field.removeAttribute("aria-invalid");
  updatePriorityLabel();
  renderAccentPicker();
  elements.form.hidden = false;
  elements.surface.focus();
  elements.form.scrollIntoView({ behavior: "smooth", block: "start" });
}

function closeEditor() {
  releasePreview();
  elements.form.hidden = true;
  elements.form.reset();
}

function updatePriorityLabel() {
  elements.priorityValue.value = elements.priority.value;
  elements.priorityValue.textContent = elements.priority.value;
}

function updateAccentFromSlider() {
  const moraCount = Number(elements.accentSlider.max) - 1;
  const sliderValue = Number(elements.accentSlider.value);
  elements.accentType.value = String(sliderValueToAccentType(moraCount, sliderValue));
  renderAccentPicker();
}

function renderAccentPicker() {
  const pronunciation = elements.pronunciation.value;
  const isValid = katakanaPronunciationPattern.test(pronunciation);
  elements.accentDiagram.replaceChildren();
  elements.accentLabels.replaceChildren();
  elements.accentEmpty.hidden = isValid;
  elements.accentPicker.hidden = !isValid;
  if (!isValid) {
    elements.accentEmpty.textContent = pronunciation
      ? "全角カタカナで入力すると選択できます。"
      : "読みを入力すると選択できます。";
    return;
  }

  const moras = splitPronunciationMoras(pronunciation);
  let accentType = Number(elements.accentType.value);
  if (!Number.isInteger(accentType) || accentType < 0 || accentType > moras.length) {
    accentType = 0;
    elements.accentType.value = "0";
  }
  const sliderMax = moras.length + 1;
  elements.accentSlider.max = String(sliderMax);
  elements.accentSlider.value = String(accentTypeToSliderValue(moras.length, accentType));
  elements.accentSlider.setAttribute("aria-valuetext", accentValueText(accentType));

  const levels = accentPitchLevels(moras.length, accentType);
  elements.accentTrack.style.setProperty("--accent-columns", String(levels.length));
  elements.accentTrack.style.setProperty("--accent-slider-margin", `${50 / levels.length}%`);
  elements.accentTrack.style.setProperty("--accent-slider-width", `${100 - (100 / levels.length)}%`);
  elements.accentTrack.style.setProperty("--accent-label-size", `${90 / levels.length}cqw`);

  const svg = document.createElementNS(svgNamespace, "svg");
  svg.classList.add("accent-line");
  svg.setAttribute("height", "44");
  svg.setAttribute("aria-hidden", "true");
  const pointCoordinates = levels.map((level, index) => ({
    x: `${((index + 0.5) / levels.length) * 100}%`,
    y: level === 1 ? 10 : 34,
  }));
  for (let index = 1; index < pointCoordinates.length; index += 1) {
    const previous = pointCoordinates[index - 1];
    const current = pointCoordinates[index];
    const line = document.createElementNS(svgNamespace, "line");
    line.setAttribute("x1", previous.x);
    line.setAttribute("y1", String(previous.y));
    line.setAttribute("x2", current.x);
    line.setAttribute("y2", String(current.y));
    svg.append(line);
  }
  for (const [index, { x, y }] of pointCoordinates.entries()) {
    const point = document.createElementNS(svgNamespace, "circle");
    point.setAttribute("cx", x);
    point.setAttribute("cy", String(y));
    point.setAttribute("r", index === pointCoordinates.length - 1 ? "4" : "5");
    if (index === pointCoordinates.length - 1) point.classList.add("is-after-word");
    svg.append(point);
  }

  for (const mora of moras) {
    const label = document.createElement("span");
    label.textContent = mora;
    elements.accentLabels.append(label);
  }
  const flatLabel = document.createElement("span");
  flatLabel.className = "is-flat";
  flatLabel.textContent = "平板";
  elements.accentLabels.append(flatLabel);
  elements.accentDiagram.append(svg);
}

function releasePreview() {
  previewAbortController?.abort();
  previewAbortController = undefined;
  previewAudio?.pause();
  previewAudio = undefined;
  if (previewAudioUrl) URL.revokeObjectURL(previewAudioUrl);
  previewAudioUrl = undefined;
}

async function previewWord() {
  if (!elements.form.reportValidity() || busy) return;
  releasePreview();
  const controller = new AbortController();
  previewAbortController = controller;
  setBusy(true);
  elements.preview.textContent = "試聴を準備中…";
  setMessage();
  try {
    const response = await fetch("/api/user-dict/preview", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      signal: controller.signal,
      body: JSON.stringify({
        pronunciation: elements.pronunciation.value.trim(),
        accent_type: Number(elements.accentType.value),
      }),
    });
    if (!response.ok) throw new Error(await readError(response));
    const blob = await response.blob();
    if (previewAbortController !== controller) return;
    previewAudioUrl = URL.createObjectURL(blob);
    previewAudio = new Audio(previewAudioUrl);
    previewAudio.addEventListener("ended", releasePreview, { once: true });
    previewAudio.addEventListener("error", () => {
      releasePreview();
      setMessage("試聴音声を再生できませんでした。", true);
    }, { once: true });
    await previewAudio.play();
    if (previewAbortController !== controller) return;
    setMessage("入力中の読みとアクセントで試聴しています。辞書には保存されていません。");
  } catch (error) {
    if (error.name === "AbortError") return;
    releasePreview();
    setMessage(error.message || "ユーザー辞書を試聴できませんでした。", true);
  } finally {
    elements.preview.textContent = "この読みで試聴";
    setBusy(false);
  }
}

function requestWord() {
  return {
    surface: elements.surface.value.trim(),
    pronunciation: elements.pronunciation.value.trim(),
    accent_type: Number(elements.accentType.value),
    word_type: elements.wordType.value,
    priority: Number(elements.priority.value),
  };
}

async function saveWord(event) {
  event.preventDefault();
  if (!elements.form.reportValidity() || busy) return;
  const uuid = elements.uuid.value;
  setBusy(true);
  elements.save.textContent = "保存中…";
  setMessage();
  let succeeded = false;
  try {
    const response = await fetch(uuid ? `/api/user-dict/words/${encodeURIComponent(uuid)}` : "/api/user-dict/words", {
      method: uuid ? "PUT" : "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(requestWord()),
    });
    if (!response.ok) throw new Error(await readError(response));
    succeeded = true;
  } catch (error) {
    setMessage(error.message || "ユーザー辞書へ保存できませんでした。", true);
  } finally {
    elements.save.textContent = uuid ? "変更を保存" : "辞書に追加";
    setBusy(false);
  }
  if (succeeded) {
    closeEditor();
    await loadDictionary(uuid ? "単語を更新しました。" : "単語を追加しました。");
  }
}

async function deleteWord(word) {
  if (busy || !confirm(`「${word.surface}」をユーザー辞書から削除しますか？`)) return;
  setBusy(true);
  setMessage();
  let succeeded = false;
  try {
    const response = await fetch(`/api/user-dict/words/${encodeURIComponent(word.uuid)}`, { method: "DELETE" });
    if (!response.ok) throw new Error(await readError(response));
    succeeded = true;
  } catch (error) {
    setMessage(error.message || "ユーザー辞書から削除できませんでした。", true);
  } finally {
    setBusy(false);
  }
  if (succeeded) {
    if (elements.uuid.value === word.uuid) closeEditor();
    await loadDictionary("単語を削除しました。");
  }
}

async function readError(response) {
  try {
    const body = await response.json();
    return body.error || `操作に失敗しました（${response.status}）`;
  } catch {
    return `操作に失敗しました（${response.status}）`;
  }
}
