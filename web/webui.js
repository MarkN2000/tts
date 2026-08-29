const invalidFileNameCharacters = /[\u0000-\u001f\u007f<>:"/\\|?*]/gu;

function sanitizeFileNamePart(value, maxLength, addEllipsis = false) {
  const cleaned = String(value ?? "")
    .replace(invalidFileNameCharacters, " ")
    .replace(/\s+/gu, " ")
    .trim()
    .replace(/[. ]+$/u, "");
  const characters = Array.from(cleaned);
  const truncated = characters
    .slice(0, maxLength)
    .join("")
    .trim()
    .replace(/[. ]+$/u, "");
  return addEllipsis && characters.length > maxLength ? `${truncated}…` : truncated;
}

function makeDownloadFileName(speakerName, styleName, text) {
  const speaker = sanitizeFileNamePart(speakerName, 30) || "既定モデル";
  const style = sanitizeFileNamePart(styleName, 30);
  const content = sanitizeFileNamePart(text, 20, true) || "音声";
  return [speaker, style, content].filter(Boolean).join("_") + ".ogg";
}

function playGeneratedAudio(audio) {
  void audio.play().catch(() => {});
}

function resizeTextArea(textarea) {
  textarea.style.height = "auto";
  const borderHeight = textarea.offsetHeight - textarea.clientHeight;
  textarea.style.height = `${textarea.scrollHeight + borderHeight}px`;
}

function makeTtsRequestUrl(engine, text, speaker) {
  const parameters = new URLSearchParams({ text });
  if (speaker !== "") parameters.set("speaker", speaker);
  return `/api/webui/${encodeURIComponent(engine)}/tts?${parameters}`;
}

function parseTtsResponse(responseText) {
  const url = new URL(responseText, "http://localhost");
  const license = url.searchParams.get("license");
  if (!responseText || !license) throw new Error("音声生成の応答が不正です");
  return { url: responseText, license };
}

const elements = {
  status: document.querySelector("#status"),
  form: document.querySelector("#tts-form"),
  engine: document.querySelector("#engine-select"),
  speaker: document.querySelector("#speaker-select"),
  text: document.querySelector("#text-input"),
  generate: document.querySelector("#generate-button"),
  result: document.querySelector("#result"),
  license: document.querySelector("#license"),
  audio: document.querySelector("#audio-player"),
  download: document.querySelector("#download-link"),
};

elements.form.addEventListener("submit", generateAudio);
elements.engine.addEventListener("change", selectEngine);
elements.text.addEventListener("input", resizeTextInput);
window.addEventListener("resize", resizeTextInput);
resizeTextInput();

let selectedEngine = "";
let speakerLoadSequence = 0;

loadEngines();

function resizeTextInput() {
  resizeTextArea(elements.text);
}

async function loadEngines() {
  setFormEnabled(false);
  setStatus("設定を読み込んでいます…");
  try {
    const response = await fetch("/api/settings", { cache: "no-store" });
    if (!response.ok) throw new Error(await readError(response));
    const settings = await response.json();
    const engines = Array.isArray(settings.engines) ? settings.engines : [];
    if (engines.length === 0 || engines.some((engine) => (
      !engine || typeof engine.id !== "string" || engine.id === ""
      || typeof engine.name !== "string" || engine.name === ""
    ))) {
      throw new Error("設定情報の応答が不正です");
    }
    populateEngines(engines);
    const requestedEngine = new URLSearchParams(window.location.search).get("engine");
    if (requestedEngine && !engines.some((engine) => engine.id === requestedEngine)) {
      throw new Error(`指定された音声エンジン「${requestedEngine}」は利用できません`);
    }
    elements.engine.value = requestedEngine || engines[0].id;
    await loadSpeakers(elements.engine.value);
  } catch (error) {
    setFormEnabled(false);
    setStatus(error.message || "設定を読み込めませんでした", true);
  }
}

function populateEngines(engines) {
  elements.engine.replaceChildren();
  for (const engine of engines) {
    const option = document.createElement("option");
    option.value = engine.id;
    option.textContent = engine.name;
    elements.engine.append(option);
  }
}

async function selectEngine() {
  clearResult();
  await loadSpeakers(elements.engine.value);
}

async function loadSpeakers(engine) {
  const sequence = ++speakerLoadSequence;
  selectedEngine = engine;
  clearSpeakers();
  setFormEnabled(false);
  setStatus("話者一覧を読み込んでいます…");
  try {
    const response = await fetch(`/api/webui/${encodeURIComponent(engine)}/speakers`, { cache: "no-store" });
    if (!response.ok) throw new Error(await readError(response));
    const speakers = await response.json();
    if (sequence !== speakerLoadSequence) return;
    populateSpeakers(speakers);
    setFormEnabled(true);
    setStatus("");
  } catch (error) {
    if (sequence !== speakerLoadSequence) return;
    setStatus(error.message || "話者一覧を読み込めませんでした", true);
  }
}

function clearSpeakers() {
  elements.speaker.replaceChildren();
  const option = document.createElement("option");
  option.value = "";
  option.textContent = "既定の話者・スタイル";
  elements.speaker.append(option);
}

function populateSpeakers(speakers) {
  for (const speaker of speakers) {
    if (!Array.isArray(speaker.styles)) continue;
    const speakerName = String(speaker.name ?? "名称未設定の話者");
    const group = document.createElement("optgroup");
    group.label = speakerName;

    for (const style of speaker.styles) {
      if (style.id === undefined || style.id === null) continue;
      const styleName = String(style.name ?? `スタイル ${style.id}`);
      const option = document.createElement("option");
      option.value = String(style.id);
      option.textContent = `${speakerName} / ${styleName}`;
      option.dataset.speakerName = speakerName;
      option.dataset.styleName = styleName;
      group.append(option);
    }

    if (group.childElementCount > 0) elements.speaker.append(group);
  }
}

async function generateAudio(event) {
  event.preventDefault();
  if (!elements.form.reportValidity()) return;

  const text = elements.text.value;
  const speaker = elements.speaker.value;
  setGenerating(true);
  setStatus("音声を生成しています…");
  clearResult();

  try {
    const response = await fetch(makeTtsRequestUrl(selectedEngine, text, speaker), {
      method: "POST",
    });
    if (!response.ok) throw new Error(await readError(response));
    const result = parseTtsResponse(await response.text());
    const selected = elements.speaker.selectedOptions[0];
    const fileName = makeDownloadFileName(
      selected?.dataset.speakerName,
      selected?.dataset.styleName,
      text,
    );
    showResult(result, fileName);
    setStatus("音声を生成しました");
  } catch (error) {
    setStatus(error.message || "音声を生成できませんでした", true);
  } finally {
    setGenerating(false);
  }
}

function showResult(result, fileName) {
  elements.license.textContent = `ライセンス: ${result.license}`;
  elements.audio.src = result.url;
  elements.download.href = result.url;
  elements.download.download = fileName;
  elements.result.hidden = false;
  playGeneratedAudio(elements.audio);
}

function setFormEnabled(enabled) {
  elements.engine.disabled = !enabled;
  elements.speaker.disabled = !enabled;
  elements.text.disabled = !enabled;
  elements.generate.disabled = !enabled;
}

function setGenerating(generating) {
  elements.engine.disabled = generating;
  elements.speaker.disabled = generating;
  elements.text.disabled = generating;
  elements.generate.disabled = generating;
  elements.generate.textContent = generating ? "生成中…" : "音声を生成";
}

function clearResult() {
  elements.result.hidden = true;
  elements.audio.removeAttribute("src");
  elements.audio.load();
}

async function readError(response) {
  const fallback = `操作に失敗しました（${response.status}）`;
  try {
    if (response.headers.get("Content-Type")?.includes("application/json")) {
      const body = await response.json();
      return body.error || fallback;
    }
    return (await response.text()) || fallback;
  } catch {
    return fallback;
  }
}

function setStatus(message, isError = false) {
  elements.status.textContent = message;
  elements.status.classList.toggle("error", isError);
}
