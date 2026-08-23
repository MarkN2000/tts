const elements = {
  status: document.querySelector("#status"),
  form: document.querySelector("#tts-form"),
  speaker: document.querySelector("#speaker-select"),
  text: document.querySelector("#text-input"),
  generate: document.querySelector("#generate-button"),
  result: document.querySelector("#result"),
  license: document.querySelector("#license"),
  audio: document.querySelector("#audio-player"),
  download: document.querySelector("#download-link"),
};

elements.form.addEventListener("submit", generateAudio);
loadSpeakers();

async function loadSpeakers() {
  setFormEnabled(false);
  setStatus("話者一覧を読み込んでいます…");
  try {
    const response = await fetch("/speakers", { cache: "no-store" });
    if (!response.ok) throw new Error(await readError(response));
    const speakers = await response.json();
    populateSpeakers(speakers);
    setFormEnabled(true);
    setStatus("");
  } catch (error) {
    setStatus(error.message || "話者一覧を読み込めませんでした", true);
  }
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
      group.append(option);
    }

    if (group.childElementCount > 0) elements.speaker.append(group);
  }
}

async function generateAudio(event) {
  event.preventDefault();
  if (!elements.form.reportValidity()) return;

  const text = elements.text.value;
  const id = elements.speaker.value;
  const body = id === "" ? { text } : { text, id };
  setGenerating(true);
  setStatus("音声を生成しています…");
  elements.result.hidden = true;
  elements.audio.removeAttribute("src");
  elements.audio.load();

  try {
    const response = await fetch("/api/webui/tts", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!response.ok) throw new Error(await readError(response));
    const result = await response.json();
    if (typeof result.url !== "string" || typeof result.license !== "string") {
      throw new Error("音声生成の応答が不正です");
    }
    showResult(result);
    setStatus("音声を生成しました");
  } catch (error) {
    setStatus(error.message || "音声を生成できませんでした", true);
  } finally {
    setGenerating(false);
  }
}

function showResult(result) {
  elements.license.textContent = `ライセンス: ${result.license}`;
  elements.audio.src = result.url;
  elements.download.href = result.url;
  elements.download.download = "tts.ogg";
  elements.result.hidden = false;
}

function setFormEnabled(enabled) {
  elements.speaker.disabled = !enabled;
  elements.text.disabled = !enabled;
  elements.generate.disabled = !enabled;
}

function setGenerating(generating) {
  elements.speaker.disabled = generating;
  elements.text.disabled = generating;
  elements.generate.disabled = generating;
  elements.generate.textContent = generating ? "生成中…" : "音声を生成";
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
