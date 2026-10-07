// Интерфейс ZapretTG. Вся логика — в Rust (src-tauri/src), здесь только отображение.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const MAX_DOM_LINES = 4000;

// ───────── уведомления ─────────
function toast(text, kind = "") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = text;
  $("toasts").appendChild(el);
  setTimeout(() => { el.classList.add("hide"); setTimeout(() => el.remove(), 350); }, kind === "error" ? 6500 : 3000);
}

async function call(cmd, args = {}, okText) {
  try {
    const r = await invoke(cmd, args);
    if (okText) toast(okText, "ok");
    return r;
  } catch (e) {
    toast(String(e), "error");
    throw e;
  }
}
const fire = (cmd, args, okText) => call(cmd, args, okText).catch(() => {}).finally(refresh);

async function copyText(text, what = "Скопировано") {
  try {
    await navigator.clipboard.writeText(text);
    toast(what, "ok");
  } catch {
    // запасной путь
    const ta = document.createElement("textarea");
    ta.value = text; document.body.appendChild(ta); ta.select();
    document.execCommand("copy"); ta.remove();
    toast(what, "ok");
  }
}

// ───────── навигация ─────────
document.querySelectorAll(".nav-item").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll(".nav-item").forEach((x) => x.classList.toggle("active", x === b));
    document.querySelectorAll(".page").forEach((p) => p.classList.toggle("active", p.id === `page-${b.dataset.page}`));
    if (b.dataset.page === "deps") loadDeps();
    scrollLog(b.dataset.page === "deps" ? "app" : b.dataset.page, true);
  })
);
document.querySelectorAll("[data-url]").forEach((a) =>
  a.addEventListener("click", () => fire("open_url", { url: a.dataset.url }))
);

// ───────── логи ─────────
const autoscroll = { zapret: () => $("z-autoscroll").checked, tg: () => $("t-autoscroll").checked, app: () => true };

function scrollLog(ch, force) {
  const box = $(`log-${ch}`);
  if (box && (force || autoscroll[ch]())) box.scrollTop = box.scrollHeight;
}

function renderLine(line) {
  const box = $(`log-${line.channel}`);
  if (!box) return;
  const el = document.createElement("div");
  if (!line.text) {
    el.className = "gap";
  } else {
    el.className = `l ${line.level}`;
    const t = document.createElement("span");
    t.className = "t";
    t.textContent = `[${line.time}]`;
    el.appendChild(t);
    el.appendChild(document.createTextNode(line.text));
  }
  box.appendChild(el);
  while (box.childElementCount > MAX_DOM_LINES) box.firstElementChild.remove();
  scrollLog(line.channel);
}

function logText(ch) {
  return [...$(`log-${ch}`).children].map((d) => d.textContent.replace(/^(\[\d\d:\d\d:\d\d\])/, "$1 ")).join("\n");
}

document.querySelectorAll("[data-log-copy]").forEach((b) =>
  b.addEventListener("click", () => copyText(logText(b.dataset.logCopy), "Лог скопирован"))
);
document.querySelectorAll("[data-log-clear]").forEach((b) =>
  b.addEventListener("click", async () => {
    await call("clear_logs", { channel: b.dataset.logClear });
    $(`log-${b.dataset.logClear}`).innerHTML = "";
  })
);
document.querySelectorAll("[data-log-save]").forEach((b) =>
  b.addEventListener("click", async () => {
    const path = await call("save_logs", { channel: b.dataset.logSave });
    toast(`Сохранено: ${path}`, "ok");
  })
);

// ───────── статусы ─────────
function setPill(id, cls, text) {
  const p = $(id);
  p.className = `pill ${cls}`;
  p.querySelector("b").textContent = text;
}
function setDot(id, cls) { $(id).className = `dot ${cls}`; }

let lastStrategies = "";

function renderZapret(s) {
  const busy = s.busy === "search" ? "Проверка конфигураций…" : s.busy === "update" ? "Обновление…" : null;
  const state = busy ? ["busy", busy] : s.running ? ["on", "Работает"] : !s.installed ? ["bad", "Не установлен"] : ["", "Остановлен"];
  setPill("z-pill", state[0], state[1]);
  setDot("nav-dot-zapret", state[0]);

  $("z-version").textContent = s.version || "—";
  $("z-latest").textContent = s.latest || "—";
  $("z-strategy").textContent = s.running_strategy || s.selected || "не выбрана";
  $("z-count").textContent = s.installed ? String(s.strategies.length) : "—";

  const toggle = $("z-toggle");
  toggle.textContent = s.running ? "Остановить" : "Запустить";
  toggle.classList.toggle("gold", !s.running);
  toggle.classList.toggle("danger", s.running);
  toggle.disabled = !!s.busy || !s.installed;
  $("z-restart").disabled = !!s.busy || !s.running;
  $("z-update").disabled = !!s.busy;

  const search = $("z-search");
  if (s.busy === "search") {
    search.textContent = "Отменить проверку";
    search.disabled = false;
  } else {
    search.textContent = "Проверить конфигурации";
    search.disabled = !!s.busy || !s.installed;
  }

  $("z-full").checked = s.full_scan;
  $("z-game").checked = s.game_filter;
  $("z-auto").checked = s.autostart;

  const pr = s.progress;
  $("z-progress").hidden = !pr;
  if (pr) {
    $("z-progress-text").textContent = `Проверяется ${pr.current} из ${pr.total} конфигураций`;
    $("z-progress-name").textContent = pr.name;
    $("z-progress-bar").style.width = `${Math.round((pr.current / Math.max(pr.total, 1)) * 100)}%`;
  }

  const key = s.strategies.join("|");
  if (key !== lastStrategies) {
    lastStrategies = key;
    const sel = $("z-select");
    sel.innerHTML = "";
    s.strategies.forEach((n) => sel.add(new Option(n, n)));
  }
  if (s.selected && document.activeElement !== $("z-select")) $("z-select").value = s.selected;
  $("z-apply").disabled = !!s.busy || !s.installed;
}

let portEdited = false;
function renderTg(s) {
  const busy = s.busy === "update" ? "Обновление…" : null;
  const state = busy
    ? ["busy", busy]
    : s.listening ? ["on", "Работает"]
    : s.running ? ["busy", "Запускается…"]
    : !s.installed ? ["bad", "Не установлен"] : ["", "Остановлен"];
  setPill("t-pill", state[0], state[1]);
  setDot("nav-dot-tg", state[0]);

  $("t-ip").textContent = s.host;
  $("t-port").textContent = String(s.port);
  $("t-secret").textContent = s.secret;
  $("t-link").textContent = s.link;
  $("t-version").textContent = s.version ? (s.latest && s.latest !== s.version ? `${s.version} (доступна ${s.latest})` : s.version) : "—";
  if (!portEdited) $("t-port-input").value = s.port;
  $("t-auto").checked = s.autostart;

  const toggle = $("t-toggle");
  toggle.textContent = s.running ? "Остановить" : "Запустить";
  toggle.classList.toggle("gold", !s.running);
  toggle.classList.toggle("danger", s.running);
  toggle.disabled = !!s.busy || !s.installed;
  $("t-restart").disabled = !!s.busy || !s.running;
  $("t-update").disabled = !!s.busy;
}

let refreshing = false;
async function refresh() {
  if (refreshing) return;
  refreshing = true;
  try {
    const [z, t] = await Promise.all([invoke("zapret_status"), invoke("tg_status")]);
    renderZapret(z);
    renderTg(t);
    window.__z = z; window.__t = t;
  } catch (e) {
    console.error(e);
  } finally {
    refreshing = false;
  }
}

// ───────── Zapret: действия ─────────
$("z-toggle").addEventListener("click", () =>
  window.__z?.running ? fire("zapret_stop") : fire("zapret_start", { name: null })
);
$("z-restart").addEventListener("click", () => fire("zapret_restart"));
$("z-search").addEventListener("click", () =>
  window.__z?.busy === "search" ? fire("zapret_cancel") : fire("zapret_search")
);
$("z-update").addEventListener("click", () => fire("zapret_update"));
$("z-full").addEventListener("change", (e) => fire("zapret_set_options", { gameFilter: null, fullScan: e.target.checked, autostart: null }));
$("z-game").addEventListener("change", (e) => {
  fire("zapret_set_options", { gameFilter: e.target.checked, fullScan: null, autostart: null });
  if (window.__z?.running) toast("Перезапустите zapret, чтобы применить Game Filter");
});
$("z-auto").addEventListener("change", (e) => fire("zapret_set_options", { gameFilter: null, fullScan: null, autostart: e.target.checked }));
$("z-apply").addEventListener("click", async () => {
  const name = $("z-select").value;
  if (!name) return;
  await call("zapret_select", { name }).catch(() => {});
  fire("zapret_start", { name });
});

// ───────── TG Proxy: действия ─────────
$("t-toggle").addEventListener("click", () => (window.__t?.running ? fire("tg_stop") : fire("tg_start")));
$("t-restart").addEventListener("click", () => fire("tg_restart"));
$("t-update").addEventListener("click", () => fire("tg_update"));
$("t-copy-link").addEventListener("click", () => copyText($("t-link").textContent, "Ссылка скопирована"));
$("t-open-tg").addEventListener("click", () => fire("tg_open_telegram"));
document.querySelectorAll("[data-copy]").forEach((b) =>
  b.addEventListener("click", () => copyText($(b.dataset.copy).textContent))
);
$("t-port-input").addEventListener("input", () => (portEdited = true));
$("t-port-apply").addEventListener("click", async () => {
  const port = parseInt($("t-port-input").value, 10);
  if (!(port >= 1024 && port <= 65535)) return toast("Порт должен быть от 1024 до 65535", "error");
  portEdited = false;
  fire("tg_set_port", { port }, "Порт сохранён");
});
$("t-regen").addEventListener("click", () => {
  if (confirm("Создать новый secret? Старая ссылка в Telegram перестанет работать.")) fire("tg_regen_secret", {}, "Новый secret создан");
});
$("t-auto").addEventListener("change", (e) => fire("tg_set_autostart", { enabled: e.target.checked }));

// ───────── компоненты ─────────
async function loadDeps() {
  const list = await invoke("deps_list");
  const box = $("deps-list");
  box.innerHTML = "";
  let anyMissing = false;
  for (const c of list) {
    if (c.required && !c.installed) anyMissing = true;
    const row = document.createElement("div");
    row.className = "dep";
    const markCls = c.installed ? "ok" : c.required ? "no" : "opt";
    row.innerHTML = `
      <div class="mark ${markCls}">${c.installed ? "✓" : c.required ? "✗" : "–"}</div>
      <div class="body">
        <div class="name"></div>
        <div class="why"></div>
        <div class="src"></div>
      </div>`;
    row.querySelector(".name").textContent = c.name + (c.version ? ` · ${c.version}` : "");
    row.querySelector(".why").textContent = c.installed ? `Установлен. ${c.why}` : c.required ? `Отсутствует. ${c.why}` : `Не установлен. ${c.why}`;
    row.querySelector(".src").textContent = `Источник: ${c.download_url}`;
    const page = document.createElement("button");
    page.className = "btn sm ghost";
    page.textContent = "Официальная страница";
    page.onclick = () => fire("open_url", { url: c.page_url });
    row.appendChild(page);
    if (!c.installed) {
      const inst = document.createElement("button");
      inst.className = `btn sm ${c.required ? "gold" : ""}`;
      inst.textContent = "Скачать и установить";
      inst.onclick = async () => {
        if (!confirm(`Скачать официальный установщик ${c.name}?\n\nИсточник: ${c.download_url}\n\nПеред запуском будет проверена цифровая подпись Microsoft.`)) return;
        inst.disabled = true;
        inst.textContent = "Установка…";
        await call("deps_install", { id: c.id }, `${c.name} установлен`).catch(() => {});
        loadDeps();
      };
      row.appendChild(inst);
    }
    box.appendChild(row);
  }
  setDot("nav-dot-deps", anyMissing ? "bad" : "on");
}
$("deps-recheck").addEventListener("click", () => loadDeps().then(() => toast("Проверка выполнена", "ok")));
$("deps-open-all").addEventListener("click", () => fire("deps_open_all_missing"));
$("s-check-updates").addEventListener("change", (e) => fire("set_check_updates", { enabled: e.target.checked }));
$("open-logs").addEventListener("click", () => fire("open_logs_folder"));
$("open-zapret").addEventListener("click", () => fire("zapret_open_folder"));

// ───────── загрузки ─────────
let dlTimer;
function onDownload({ what, done, total }) {
  $("download").hidden = false;
  $("download-name").textContent = `${what} — ${(done / 1048576).toFixed(1)} / ${(total / 1048576).toFixed(1)} МБ`;
  $("download-bar").style.width = `${total ? Math.round((done / total) * 100) : 0}%`;
  clearTimeout(dlTimer);
  dlTimer = setTimeout(() => ($("download").hidden = true), done >= total ? 1500 : 20000);
}

// ───────── старт ─────────
(async function init() {
  const info = await invoke("app_info");
  $("app-version").textContent = `v${info.version} · portable`;
  $("admin-warn").hidden = info.elevated;
  $("s-check-updates").checked = info.check_updates_on_start;
  $("s-data").textContent = info.data_dir;

  for (const ch of ["zapret", "tg", "app"]) {
    (await invoke("get_logs", { channel: ch })).forEach(renderLine);
  }
  await listen("log", (e) => renderLine(e.payload));
  let pending;
  await listen("status-changed", () => { clearTimeout(pending); pending = setTimeout(refresh, 80); });
  await listen("download-progress", (e) => onDownload(e.payload));

  await refresh();
  loadDeps();
  setInterval(refresh, 2000);
})();
