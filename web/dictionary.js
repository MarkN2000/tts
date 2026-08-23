const typeLabels = {
  PROPER_NOUN: "固有名詞",
  COMMON_NOUN: "普通名詞",
  VERB: "動詞",
  ADJECTIVE: "形容詞",
  SUFFIX: "接尾辞",
};

const elements = {
  status: document.querySelector("#status"),
  excluded: document.querySelector("#excluded-note"),
  editor: document.querySelector("#editor"),
  title: document.querySelector("#editor-title"),
  form: document.querySelector("#word-form"),
  uuid: document.querySelector("#word-uuid"),
  surface: document.querySelector("#surface"),
  pronunciation: document.querySelector("#pronunciation"),
  wordType: document.querySelector("#word-type"),
  accentType: document.querySelector("#accent-type"),
  priority: document.querySelector("#priority"),
  list: document.querySelector("#word-list"),
  count: document.querySelector("#word-count"),
  empty: document.querySelector("#empty-message"),
  reload: document.querySelector("#reload-button"),
  preview: document.querySelector("#preview-button"),
  save: document.querySelector("#save-button"),
};

let words = [];
let previewAudio;
let previewAudioUrl;
let previewAbortController;

document.querySelector("#add-button").addEventListener("click", () => openEditor());
elements.reload.addEventListener("click", () => loadDictionary(true));
document.querySelector("#cancel-button").addEventListener("click", closeEditor);
elements.form.addEventListener("submit", saveWord);
elements.preview.addEventListener("click", previewWord);

loadDictionary();

async function loadDictionary(showSuccess = false) {
  elements.reload.disabled = true;
  elements.reload.textContent = "読み込み中…";
  setStatus("読み込み中です…");
  try {
    const response = await fetch("/api/user-dict", { cache: "no-store" });
    if (!response.ok) throw new Error(await readError(response));
    const dictionary = await response.json();
    words = dictionary.words;
    elements.excluded.hidden = !dictionary.has_excluded_words;
    renderWords();
    setStatus(showSuccess ? "最新の辞書を読み込みました" : "");
  } catch (error) {
    setStatus(error.message, true);
  } finally {
    elements.reload.disabled = false;
    elements.reload.textContent = "辞書を再読み込み";
  }
}

function renderWords() {
  elements.list.replaceChildren();
  elements.count.textContent = `${words.length}件`;
  elements.empty.hidden = words.length !== 0;

  for (const word of words) {
    const row = document.createElement("article");
    row.className = "word-row";

    const main = document.createElement("div");
    main.className = "word-main";
    const surface = document.createElement("p");
    surface.className = "word-surface";
    surface.textContent = word.surface;
    const meta = document.createElement("p");
    meta.className = "word-meta";
    meta.textContent = `${word.pronunciation} · ${typeLabels[word.word_type]} · アクセント ${word.accent_type} · 優先度 ${word.priority}`;
    main.append(surface, meta);

    const actions = document.createElement("div");
    actions.className = "word-actions";
    const edit = document.createElement("button");
    edit.type = "button";
    edit.className = "secondary";
    edit.textContent = "編集";
    edit.addEventListener("click", () => openEditor(word));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "danger";
    remove.textContent = "削除";
    remove.addEventListener("click", () => deleteWord(word));
    actions.append(edit, remove);
    row.append(main, actions);
    elements.list.append(row);
  }
}

function openEditor(word = null) {
  releasePreview();
  elements.form.reset();
  elements.uuid.value = word?.uuid ?? "";
  elements.surface.value = word?.surface ?? "";
  elements.pronunciation.value = word?.pronunciation ?? "";
  elements.wordType.value = word?.word_type ?? "PROPER_NOUN";
  elements.accentType.value = word?.accent_type ?? 0;
  elements.priority.value = word?.priority ?? 5;
  elements.title.textContent = word ? "単語を編集" : "単語を追加";
  elements.editor.hidden = false;
  elements.surface.focus();
  elements.editor.scrollIntoView({ behavior: "smooth", block: "start" });
}

function closeEditor() {
  releasePreview();
  elements.editor.hidden = true;
  elements.form.reset();
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
  if (!elements.form.reportValidity()) return;
  releasePreview();
  const controller = new AbortController();
  previewAbortController = controller;
  elements.preview.disabled = true;
  elements.preview.textContent = "試聴を準備中…";
  setStatus("");
  try {
    const response = await fetch("/api/user-dict/preview", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      signal: controller.signal,
      body: JSON.stringify({
        pronunciation: elements.pronunciation.value,
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
      setStatus("試聴音声を再生できませんでした", true);
    }, { once: true });
    await previewAudio.play();
    if (previewAbortController !== controller) return;
    setStatus("入力中の読みとアクセントで試聴しています。辞書には保存されていません");
  } catch (error) {
    if (error.name === "AbortError") return;
    releasePreview();
    setStatus(error.message || "試聴できませんでした", true);
  } finally {
    elements.preview.disabled = false;
    elements.preview.textContent = "この読みで試聴";
  }
}

async function saveWord(event) {
  event.preventDefault();
  const uuid = elements.uuid.value;
  const body = {
    surface: elements.surface.value,
    pronunciation: elements.pronunciation.value,
    accent_type: Number(elements.accentType.value),
    word_type: elements.wordType.value,
    priority: Number(elements.priority.value),
  };
  elements.save.disabled = true;
  setStatus("保存中です…");
  try {
    const response = await fetch(uuid ? `/api/user-dict/words/${uuid}` : "/api/user-dict/words", {
      method: uuid ? "PUT" : "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!response.ok) throw new Error(await readError(response));
    closeEditor();
    await loadDictionary();
  } catch (error) {
    setStatus(error.message, true);
  } finally {
    elements.save.disabled = false;
  }
}

async function deleteWord(word) {
  if (!confirm(`「${word.surface}」を削除しますか？`)) return;
  setStatus("削除中です…");
  try {
    const response = await fetch(`/api/user-dict/words/${word.uuid}`, { method: "DELETE" });
    if (!response.ok) throw new Error(await readError(response));
    if (elements.uuid.value === word.uuid) closeEditor();
    await loadDictionary();
  } catch (error) {
    setStatus(error.message, true);
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

function setStatus(message, isError = false) {
  elements.status.textContent = message;
  elements.status.classList.toggle("error", isError);
}
