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

function formatByteSize(bytes) {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const digits = unitIndex === 0 || value >= 10 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unitIndex]}`;
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
  currentVersion: document.querySelector("#current-version"),
  checkUpdate: document.querySelector("#check-update-button"),
  applyUpdate: document.querySelector("#apply-update-button"),
  updateStatus: document.querySelector("#update-status"),
  updateError: document.querySelector("#update-error"),
  restartPanel: document.querySelector("#restart-panel"),
  restart: document.querySelector("#restart-button"),
  restartStatus: document.querySelector("#restart-status"),
  restartError: document.querySelector("#restart-error"),
  engine: document.querySelector("#dictionary-engine-select"),
  engineLinks: document.querySelector("#engine-links"),
  linksError: document.querySelector("#links-error"),
  cacheUsage: document.querySelector("#cache-usage"),
  cacheFileCount: document.querySelector("#cache-file-count"),
  cacheDays: document.querySelector("#cache-days"),
  clearCache: document.querySelector("#clear-cache-button"),
  cacheStatus: document.querySelector("#cache-status"),
  cacheError: document.querySelector("#cache-error"),
};

let words = [];
let busy = false;
let updateBusy = false;
let availableVersion;
let currentInstanceId = "";
let previewAudio;
let previewAudioUrl;
let previewAbortController;
let selectedEngine = "";

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
elements.checkUpdate.addEventListener("click", checkForUpdate);
elements.applyUpdate.addEventListener("click", applyUpdate);
elements.restart.addEventListener("click", restartServer);
elements.clearCache.addEventListener("click", clearCache);
elements.engine.addEventListener("change", selectEngine);

for (const field of elements.form.elements) {
  field.addEventListener("invalid", () => field.setAttribute("aria-invalid", "true"));
  field.addEventListener("input", () => field.setAttribute("aria-invalid", String(!field.validity.valid)));
}

loadVersionInfo();
loadSettingsInfo();
loadCacheInfo();

function setMessage(message = "", isError = false) {
  elements.status.textContent = isError ? "" : message;
  elements.error.textContent = isError ? message : "";
}

function setBusy(value) {
  busy = value;
  updateDisabledState();
}

function setUpdateBusy(value) {
  updateBusy = value;
  updateDisabledState();
}

function updateDisabledState() {
  const disabled = busy || updateBusy;
  const dictionaryDisabled = disabled || selectedEngine === "";
  elements.engine.disabled = dictionaryDisabled;
  elements.reload.disabled = dictionaryDisabled;
  elements.add.disabled = dictionaryDisabled;
  elements.checkUpdate.disabled = disabled;
  elements.applyUpdate.disabled = disabled;
  elements.restart.disabled = disabled;
  elements.clearCache.disabled = disabled;
  for (const button of elements.list.querySelectorAll("button")) button.disabled = disabled;
  for (const field of elements.form.elements) field.disabled = dictionaryDisabled;
}

async function loadVersionInfo() {
  try {
    const response = await fetch("/api/version", { cache: "no-store" });
    if (!response.ok) throw new Error("バージョン情報を取得できませんでした。");
    const version = await response.json();
    if (typeof version.current_version !== "string" || version.current_version === ""
      || typeof version.instance_id !== "string" || version.instance_id === "") {
      throw new Error("バージョン情報の応答が不正です。");
    }
    currentInstanceId = version.instance_id;
    elements.currentVersion.textContent = `現在 v${version.current_version}`;
    elements.restartPanel.hidden = version.restart_supported !== true;
    if (!version.supported) {
      setUpdateMessage("自動アップデートはLinux x86_64版で利用できます。");
      return;
    }
    elements.checkUpdate.hidden = false;
    setUpdateMessage();
  } catch (error) {
    setUpdateMessage(error.message || "バージョン情報を取得できませんでした。", true);
  }
}

async function loadSettingsInfo() {
  try {
    const response = await fetch("/api/settings", { cache: "no-store" });
    if (!response.ok) throw new Error("公開API URLを取得できませんでした。");
    const settings = await response.json();
    if (!Array.isArray(settings.engines) || settings.engines.length === 0 || settings.engines.some((engine) => (
      !engine || typeof engine.id !== "string" || engine.id === ""
      || typeof engine.name !== "string" || engine.name === ""
      || typeof engine.public_tts_url !== "string" || engine.public_tts_url === ""
      || typeof engine.public_speakers_url !== "string" || engine.public_speakers_url === ""
    ))) {
      throw new Error("設定情報の応答が不正です。");
    }
    populateEngines(settings.engines);
    selectedEngine = elements.engine.value;
    updateDisabledState();
    elements.linksError.textContent = "";
    await loadDictionary();
  } catch (error) {
    elements.engineLinks.textContent = "取得できませんでした";
    elements.linksError.textContent = error.message || "公開API URLを取得できませんでした。";
  }
}

function populateEngines(engines) {
  elements.engine.replaceChildren();
  elements.engineLinks.replaceChildren();
  for (const engine of engines) {
    const option = document.createElement("option");
    option.value = engine.id;
    option.textContent = engine.name;
    elements.engine.append(option);

    const group = document.createElement("article");
    group.className = "engine-link-group";
    const heading = document.createElement("h3");
    heading.textContent = engine.name;
    const webui = document.createElement("a");
    webui.className = "settings-link";
    webui.href = `/webui?engine=${encodeURIComponent(engine.id)}`;
    webui.innerHTML = "<strong>音声生成 Web UI</strong><span>話者とテキストを選んで音声を生成します。</span>";
    const tts = createPublicLinkItem("公開 TTS API（GET / POST）", engine.public_tts_url);
    const speakers = createPublicLinkItem("話者一覧 API（GET）", engine.public_speakers_url);
    group.append(heading, webui, tts, speakers);
    elements.engineLinks.append(group);
  }
}

function createPublicLinkItem(label, url) {
  const item = document.createElement("div");
  item.className = "link-item";
  const title = document.createElement("strong");
  title.textContent = label;
  const value = document.createElement("code");
  value.textContent = url;
  item.append(title, value);
  return item;
}

async function selectEngine() {
  if (busy || updateBusy) return;
  selectedEngine = elements.engine.value;
  closeEditor();
  releasePreview();
  words = [];
  renderWords();
  await loadDictionary();
}

function dictionaryApiPath(path = "") {
  return `/api/engines/${encodeURIComponent(selectedEngine)}/user-dict${path}`;
}

function setCacheMessage(message = "", isError = false) {
  elements.cacheStatus.textContent = isError ? "" : message;
  elements.cacheError.textContent = isError ? message : "";
}

async function loadCacheInfo(successMessage = "") {
  try {
    const response = await fetch("/api/cache", { cache: "no-store" });
    if (!response.ok) throw new Error(await readError(response));
    const cache = await response.json();
    const values = [cache.used_bytes, cache.max_bytes, cache.file_count, cache.cache_days];
    if (!values.every((value) => Number.isSafeInteger(value) && value >= 0)) {
      throw new Error("キャッシュ情報の応答が不正です。");
    }
    elements.cacheUsage.textContent = `${formatByteSize(cache.used_bytes)} / ${formatByteSize(cache.max_bytes)}`;
    elements.cacheFileCount.textContent = `${cache.file_count}件`;
    elements.cacheDays.textContent = `${cache.cache_days}日`;
    setCacheMessage(successMessage);
  } catch (error) {
    elements.cacheUsage.textContent = "取得できませんでした";
    elements.cacheFileCount.textContent = "—";
    elements.cacheDays.textContent = "—";
    setCacheMessage(error.message || "キャッシュ情報を取得できませんでした。", true);
  }
}

async function clearCache() {
  if (busy || updateBusy || !confirm("保存済みの音声キャッシュをすべて削除しますか？")) return;
  setBusy(true);
  elements.clearCache.textContent = "削除中…";
  setCacheMessage("音声生成の完了を待ってキャッシュを削除しています…");
  try {
    const response = await fetch("/api/cache", { method: "DELETE" });
    if (!response.ok) throw new Error(await readError(response));
    await loadCacheInfo("キャッシュを削除しました。");
  } catch (error) {
    setCacheMessage(error.message || "キャッシュを削除できませんでした。", true);
  } finally {
    elements.clearCache.textContent = "キャッシュを削除";
    setBusy(false);
  }
}

function setUpdateMessage(message = "", isError = false) {
  elements.updateStatus.textContent = isError ? "" : message;
  elements.updateError.textContent = isError ? message : "";
}

function setRestartMessage(message = "", isError = false) {
  elements.restartStatus.textContent = isError ? "" : message;
  elements.restartError.textContent = isError ? message : "";
}

async function restartServer() {
  if (busy || updateBusy || currentInstanceId === "") return;
  if (!confirm("config.tomlを検証してサーバーを再起動しますか？")) return;
  const previousInstanceId = currentInstanceId;
  setUpdateBusy(true);
  setBusy(true);
  elements.restart.textContent = "検証中…";
  setRestartMessage("設定と起動に必要なTTS Engine・FFmpegを確認しています…");

  let response;
  try {
    response = await fetch("/api/restart", { method: "POST" });
  } catch {
    response = undefined;
  }
  if (response && !response.ok) {
    setRestartMessage(await readError(response), true);
    finishRestartAttempt();
    return;
  }

  elements.restart.textContent = "再起動中…";
  setRestartMessage("サーバーを再起動しています…");
  const restarted = await waitForInstance(previousInstanceId, 90_000);
  if (restarted) {
    currentInstanceId = restarted.instance_id;
    elements.currentVersion.textContent = `現在 v${restarted.current_version}`;
    setRestartMessage("設定を反映して再起動しました。");
    finishRestartAttempt();
    await loadSettingsInfo();
    await loadCacheInfo();
    return;
  }
  setRestartMessage("再起動後のサーバーへ接続できません。systemdの状態と変更後のadmin_listenを確認してください。", true);
  finishRestartAttempt();
}

async function waitForInstance(previousInstanceId, timeoutMilliseconds) {
  const deadline = Date.now() + timeoutMilliseconds;
  while (Date.now() < deadline) {
    await delay(2_000);
    try {
      const response = await fetch("/api/version", { cache: "no-store" });
      if (!response.ok) continue;
      const version = await response.json();
      if (typeof version.instance_id === "string" && version.instance_id !== ""
        && version.instance_id !== previousInstanceId
        && typeof version.current_version === "string" && version.current_version !== "") {
        return version;
      }
    } catch {
      // 再起動中の接続失敗は想定内。
    }
  }
  return undefined;
}

function finishRestartAttempt() {
  elements.restart.textContent = "設定を反映して再起動";
  setUpdateBusy(false);
  setBusy(false);
}

async function checkForUpdate() {
  if (busy || updateBusy) return;
  setUpdateBusy(true);
  availableVersion = undefined;
  elements.applyUpdate.hidden = true;
  elements.checkUpdate.textContent = "確認中…";
  setUpdateMessage("最新リリースを確認しています…");
  try {
    const response = await fetch("/api/update", { cache: "no-store" });
    if (!response.ok) throw new Error(await readError(response));
    const update = await response.json();
    elements.currentVersion.textContent = `現在 v${update.current_version}`;
    if (!update.update_available) {
      setUpdateMessage(`v${update.current_version}は最新版です。`);
      return;
    }
    availableVersion = update.latest_version;
    elements.applyUpdate.hidden = false;
    setUpdateMessage(`v${availableVersion}を利用できます。`);
  } catch (error) {
    setUpdateMessage(error.message || "アップデートを確認できませんでした。", true);
  } finally {
    elements.checkUpdate.textContent = "アップデートを確認";
    setUpdateBusy(false);
  }
}

async function applyUpdate() {
  if (busy || updateBusy || !availableVersion) return;
  if (!confirm(`v${availableVersion}へ更新してサーバーを再起動しますか？`)) return;
  let targetVersion = availableVersion;
  setUpdateBusy(true);
  setBusy(true);
  elements.applyUpdate.textContent = "更新中…";
  setUpdateMessage("更新ファイルを取得して検証しています。画面を閉じないでください。");

  let response;
  try {
    response = await fetch("/api/update", { method: "POST" });
  } catch {
    // 応答送信直後に再起動すると通信が切れることがあるため、対象版の起動も確認する。
    const running = await waitForVersion(targetVersion, 20_000);
    if (running) {
      showUpdateCompleted(targetVersion, running.instance_id);
      return;
    }
    setUpdateMessage("アップデートを実行できませんでした。", true);
    finishUpdateAttempt();
    return;
  }

  if (!response.ok) {
    setUpdateMessage(await readError(response), true);
    finishUpdateAttempt();
    return;
  }
  try {
    const result = await response.json();
    if (typeof result.version !== "string" || result.version === "") {
      throw new Error("更新後のバージョンを確認できませんでした。");
    }
    targetVersion = result.version;
  } catch (error) {
    setUpdateMessage(error.message || "更新後のバージョンを確認できませんでした。", true);
    finishUpdateAttempt();
    return;
  }
  setUpdateMessage("サーバーを再起動しています…");
  const running = await waitForVersion(targetVersion, 90_000);
  if (running) {
    showUpdateCompleted(targetVersion, running.instance_id);
    return;
  }
  setUpdateMessage("再起動後のサーバーへ接続できません。systemdの状態を確認してください。", true);
  finishUpdateAttempt();
}

async function waitForVersion(targetVersion, timeoutMilliseconds) {
  const deadline = Date.now() + timeoutMilliseconds;
  while (Date.now() < deadline) {
    await delay(2_000);
    try {
      const response = await fetch("/api/version", { cache: "no-store" });
      if (!response.ok) continue;
      const version = await response.json();
      if (version.current_version === targetVersion
        && typeof version.instance_id === "string" && version.instance_id !== "") return version;
    } catch {
      // 再起動中の接続失敗は想定内。
    }
  }
  return undefined;
}

function showUpdateCompleted(version, instanceId) {
  currentInstanceId = instanceId;
  elements.currentVersion.textContent = `現在 v${version}`;
  elements.applyUpdate.hidden = true;
  availableVersion = undefined;
  setUpdateMessage(`v${version}へ更新しました。`);
  finishUpdateAttempt();
}

function finishUpdateAttempt() {
  elements.applyUpdate.textContent = "更新して再起動";
  setUpdateBusy(false);
  setBusy(false);
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function loadDictionary(successMessage = "") {
  if (!selectedEngine) return;
  setBusy(true);
  elements.reload.textContent = "読み込み中…";
  setMessage("読み込み中です…");
  try {
    const response = await fetch(dictionaryApiPath(), { cache: "no-store" });
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
  if (busy || !selectedEngine) return;
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
    const response = await fetch(dictionaryApiPath("/preview"), {
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
    const response = await fetch(uuid ? dictionaryApiPath(`/words/${encodeURIComponent(uuid)}`) : dictionaryApiPath("/words"), {
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
    await loadCacheInfo();
  }
}

async function deleteWord(word) {
  if (busy || !confirm(`「${word.surface}」をユーザー辞書から削除しますか？`)) return;
  setBusy(true);
  setMessage();
  let succeeded = false;
  try {
    const response = await fetch(dictionaryApiPath(`/words/${encodeURIComponent(word.uuid)}`), { method: "DELETE" });
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
    await loadCacheInfo();
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
