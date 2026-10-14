const modelCopy = {
  "admin.models.title": "模型接入",
  "admin.models.loading": "正在读取模型配置…",
  "admin.models.provider_note": "OpenAI Codex 和 OpenCode 是编码客户端/编排器，不是此处的模型 API provider。OpenAI API preset 需要 API key；ChatGPT 订阅登录不能代替它。",
  "admin.models.ready": "已配置默认模型。服务商 {providers} 个，模型 {models} 个。",
  "admin.models.missing": "还没有可用的默认模型。添加服务商和模型后，客户端即可开始对话。",
  "admin.models.empty": "尚未添加模型服务。",
  "admin.models.add": "添加模型服务",
  "admin.models.provider": "模型服务商",
  "admin.models.api_key": "API key",
  "admin.models.model_id": "模型 ID",
  "admin.models.save": "保存并设为默认模型",
  "admin.models.preset_required": "请选择模型服务商。",
  "admin.models.api_key_required": "请输入 API key。",
  "admin.models.model_id_required": "请输入模型 ID。",
  "admin.models.saved": "模型服务已添加并设为默认模型。",
  "admin.models.failed": "添加模型服务失败：{error}",
  "admin.models.saving": "正在保存…",
  "admin.models.provider_key": "已保存 API key",
  "admin.models.no_key": "未配置 API key",
  "admin.models.no_provider_models": "尚未登记模型",
  "common.cancel": "取消"
};
const modelT = (key, values = {}) => (modelCopy[key] || key).replace(/\{(\w+)\}/g, (_, name) => String(values[name] ?? "—"));
document.querySelectorAll("[data-model-i18n]").forEach(node => node.textContent = modelT(node.dataset.modelI18n));

const modelState = document.getElementById("model-state");
const modelList = document.getElementById("model-list");
const modelForm = document.getElementById("model-setup-form");
const modelPreset = document.getElementById("model-preset");
const modelId = document.getElementById("model-id");
const modelApiKey = document.getElementById("model-api-key");
const modelError = document.getElementById("model-error");
const modelAddToggle = document.getElementById("model-add-toggle");
const modelSubmit = document.getElementById("model-submit");
let modelPresets = [];
let modelDataLoaded = false;

function renderModelRows(data) {
  modelList.replaceChildren();
  const providers = Array.isArray(data.providers) ? data.providers : [];
  const models = Array.isArray(data.models) ? data.models : [];
  if (!providers.length) {
    modelState.textContent = modelT("admin.models.empty");
    return;
  }
  modelState.textContent = data.model_setup_required
    ? modelT("admin.models.missing")
    : modelT("admin.models.ready", { providers: providers.length, models: models.length });
  for (const provider of providers) {
    const row = document.createElement("div");
    row.className = "model-row";
    const details = document.createElement("div");
    const name = document.createElement("strong");
    name.textContent = provider.name || provider.id || "—";
    const listedModels = models.filter(model => model.provider_id === provider.id);
    const detail = document.createElement("small");
    detail.textContent = listedModels.length
      ? listedModels.map(model => model.display_name || model.model_id).join("、")
      : modelT("admin.models.no_provider_models");
    details.append(name, detail);
    const keyState = document.createElement("small");
    keyState.textContent = provider.has_key ? modelT("admin.models.provider_key") : modelT("admin.models.no_key");
    row.append(details, keyState);
    modelList.append(row);
  }
}

function updatePresetInfo() {
  const preset = modelPresets.find(item => item.id === modelPreset.value);
  document.getElementById("model-preset-info").textContent = preset
    ? `${preset.description} API 地址：${preset.base_url}`
    : "";
  if (preset) modelId.value = preset.model_id || "";
}

async function loadModelSetup() {
  try {
    const response = await request("/admin/models/status");
    const data = await response.json();
    modelPresets = Array.isArray(data.presets) ? data.presets : [];
    modelPreset.replaceChildren();
    for (const preset of modelPresets) {
      const option = document.createElement("option");
      option.value = preset.id;
      option.textContent = preset.name;
      modelPreset.append(option);
    }
    updatePresetInfo();
    renderModelRows(data);
    modelDataLoaded = true;
  } catch (error) {
    if (error.status === 401) return;
    modelState.textContent = modelT("admin.models.failed", { error: error.message });
  }
}

modelPreset.addEventListener("change", updatePresetInfo);
modelAddToggle.addEventListener("click", () => {
  modelError.textContent = "";
  modelForm.classList.remove("hidden");
  modelAddToggle.classList.add("hidden");
  modelPreset.focus();
});
document.getElementById("model-cancel").addEventListener("click", () => {
  modelForm.reset();
  modelForm.classList.add("hidden");
  modelAddToggle.classList.remove("hidden");
  modelError.textContent = "";
});
modelForm.addEventListener("submit", async event => {
  event.preventDefault();
  const preset = modelPresets.find(item => item.id === modelPreset.value);
  if (!preset || !modelApiKey.value.trim() || !modelId.value.trim()) {
    modelError.textContent = modelT(!preset ? "admin.models.preset_required" : !modelApiKey.value.trim() ? "admin.models.api_key_required" : "admin.models.model_id_required");
    return;
  }
  modelSubmit.disabled = true;
  modelSubmit.textContent = modelT("admin.models.saving");
  modelError.textContent = "";
  try {
    await request("/admin/models/setup", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ preset_id: preset.id, api_key: modelApiKey.value, model_id: modelId.value.trim() })
    });
    modelForm.reset();
    modelForm.classList.add("hidden");
    modelAddToggle.classList.remove("hidden");
    modelApiKey.value = "";
    toast(modelT("admin.models.saved"));
    await loadModelSetup();
  } catch (error) {
    modelError.textContent = modelT("admin.models.failed", { error: error.message });
  } finally {
    modelSubmit.disabled = false;
    modelSubmit.textContent = modelT("admin.models.save");
  }
});

const modelAdminView = document.getElementById("admin-view");
let modelAdminWasVisible = false;
const modelAdminObserver = new MutationObserver(() => {
  const visible = !modelAdminView.classList.contains("hidden");
  if (visible && !modelAdminWasVisible) loadModelSetup();
  modelAdminWasVisible = visible;
});
modelAdminObserver.observe(modelAdminView, { attributes: true, attributeFilter: ["class"] });
if (!modelAdminView.classList.contains("hidden") && !modelDataLoaded) loadModelSetup();
