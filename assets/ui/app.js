"use strict";
// Scriptura 管理画面の少量の JS（CSP で同一オリジンのファイルだけを許可。インライン script は使わない）。
// 役割は文字数カウンター・選択欄とチェックボックスの自動送信・「アプリ」画面の「アプリの更新」パネルの操作・概要の「実行中」パネルの自動更新だけ。送信内容の検査はすべてサーバー側で行う（ここでの表示は目安）。
// 単位の文言は表示言語に合わせてサーバーが body の data 属性に入れる（CSP のためインライン script は使わない）。
(() => {
  const encoder = new TextEncoder();
  // 改行は保存時に LF へ揃えるので、数えるときも CRLF を 1 文字として扱う。
  const normalize = text => text.replace(/\r\n?/g, "\n");
  const units = () => ({
    chars: document.body.dataset.unitChars || "",
    bytes: document.body.dataset.unitBytes || "",
  });
  const update = (area, counter) => {
    const text = normalize(area.value);
    const maxChars = Number(area.dataset.max || 0);
    const maxBytes = Number(area.dataset.maxBytes || 0);
    if (maxChars) {
      const count = [...text].length;
      counter.textContent = `${count} / ${maxChars} ${units().chars}`;
      counter.classList.toggle("over", count > maxChars);
    } else if (maxBytes) {
      const bytes = encoder.encode(text).length;
      counter.textContent = `${bytes} / ${maxBytes} ${units().bytes}`;
      counter.classList.toggle("over", bytes > maxBytes);
    }
  };
  document.addEventListener("DOMContentLoaded", () => {
    for (const counter of document.querySelectorAll(".counter[data-for]")) {
      const form = counter.closest("form");
      const area = form && form.querySelector(`textarea[name="${counter.dataset.for}"]`);
      if (!area) continue;
      area.addEventListener("input", () => update(area, counter));
      update(area, counter);
    }
    // 言語の選択欄と更新の設定のチェックボックスは、変えた時点で送信する（JS が無い環境ではボタンで送る）。
    for (const control of document.querySelectorAll("[data-autosubmit]")) {
      const form = control.closest("form");
      const fallback = form && form.querySelector("[data-autosubmit-fallback]");
      if (fallback) fallback.hidden = true;
      control.addEventListener("change", () => form && form.submit());
    }
    const panel = document.querySelector("[data-updates]");
    if (panel) setupUpdates(panel);
    const activity = document.querySelector("[data-activity]");
    if (activity) setupActivity(activity);
  });

  // 概要の「実行中」パネル。2 秒ごとに GET /activity（サーバーが作る HTML の断片）を取り、パネルの中身だけを置き換える。
  // 経過・残りの秒は取得の間も 1 秒ごとにここで進める（サーバー時刻との差を補正）。JS が無い環境では、開いた時点の表示のまま。
  function setupActivity(panel) {
    const POLL_MS = 2000;
    const text = name => panel.dataset[name] || "";
    const note = panel.querySelector("[data-activity-stale]");
    const body = () => panel.querySelector("[data-activity-body]");
    let skew = 0; // サーバー時刻 - ブラウザー時刻（ミリ秒）
    let inflight = false;
    const pad = n => String(n).padStart(2, "0");
    // サーバーの `views::clock` と同じ形（1 時間以上は H:MM:SS、未満は M:SS）。
    const clock = total => {
      const s = Math.max(0, Math.floor(total));
      const h = Math.floor(s / 3600);
      const m = Math.floor((s % 3600) / 60);
      return h > 0 ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
    };
    const tick = () => {
      const now = Math.floor((Date.now() + skew) / 1000);
      for (const el of panel.querySelectorAll("[data-since]")) {
        const next = clock(now - Number(el.dataset.since));
        if (el.textContent !== next) el.textContent = next;
      }
      for (const el of panel.querySelectorAll("[data-until]")) {
        const next = clock(Number(el.dataset.until) - now);
        if (el.textContent !== next) el.textContent = next;
      }
    };
    // 秒だけが違う断片を「同じ」とみなすための比較用の文字列。
    const signature = element => {
      const copy = element.cloneNode(true);
      copy.removeAttribute("data-now");
      copy.removeAttribute("data-runs-latest");
      for (const el of copy.querySelectorAll("[data-since],[data-until]")) el.textContent = "";
      return copy.outerHTML;
    };
    // フォーカスやテキスト選択がパネルの中にあるあいだは、置き換えを次の回へ延ばす。
    const holding = element => {
      const active = document.activeElement;
      if (active && active !== document.body && element.contains(active)) return true;
      const selection = document.getSelection();
      return !!selection && !selection.isCollapsed &&
        (element.contains(selection.anchorNode) || element.contains(selection.focusNode));
    };
    const showNote = message => {
      if (!note) return;
      note.textContent = message || "";
      note.hidden = !message;
    };
    // 置き換えを延ばす最大の回数（2 秒おきなので約 10 秒）。超えたら、選択やフォーカスがあっても置き換える。
    const MAX_HOLDS = 5;
    let bodyHolds = 0;
    let runsHolds = 0;
    let held = false; // 保守の部分の置き換えを延ばしている間は、古い行の経過を進めない。
    // 「最近の保守」を取り直す必要があるか。成功して反映するまで覚える。
    let runsPending = false;
    const latestOf = element => (element && element.dataset.runsLatest) || "";
    const initialBody = body();
    let reflectedLatest = latestOf(initialBody);
    // 置き換えたとき、フォーカスしていたリンクと同じ href があれば、フォーカスを戻す。
    const replaceKeepingFocus = (current, next) => {
      const active = document.activeElement;
      const href = active && active !== document.body && current.contains(active) && active.tagName === "A"
        ? active.getAttribute("href") : null;
      const imported = document.importNode(next, true);
      current.replaceWith(imported);
      if (href === null) return;
      for (const link of imported.querySelectorAll("a[href]")) {
        if (link.getAttribute("href") === href) { link.focus(); break; }
      }
    };
    // 保守の部分を反映する。
    const applyBody = doc => {
      const next = doc.querySelector("[data-activity-body]");
      if (!next) throw new Error("fragment");
      const current = body();
      if (!current) return;
      if (Number(next.dataset.count) < Number(current.dataset.count)) runsPending = true;
      if (latestOf(next) !== reflectedLatest) runsPending = true;
      if (signature(next) === signature(current)) {
        skew = Number(next.dataset.now) * 1000 - Date.now();
        bodyHolds = 0; held = false; tick();
        return;
      }
      if (holding(current) && bodyHolds < MAX_HOLDS) { bodyHolds += 1; held = true; return; }
      bodyHolds = 0; held = false;
      skew = Number(next.dataset.now) * 1000 - Date.now();
      replaceKeepingFocus(current, next);
      tick();
    };
    // 「最近の保守」を反映できたら true。延ばしたときは false（次の回で再試行する）。
    const applyRuns = doc => {
      const next = doc.querySelector("[data-activity-runs]");
      const current = document.querySelector("[data-activity-runs]");
      if (!next || !current) throw new Error("fragment");
      const latest = latestOf(doc.querySelector("[data-activity-body]"));
      if (next.outerHTML !== current.outerHTML) {
        if (holding(current) && runsHolds < MAX_HOLDS) { runsHolds += 1; return false; }
        replaceKeepingFocus(current, next);
      }
      runsHolds = 0;
      reflectedLatest = latest;
      return true;
    };
    const load = async withRuns => {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), 10000);
      try {
        const response = await fetch(withRuns ? "/activity?runs=1" : "/activity", {
          cache: "no-store",
          credentials: "same-origin",
          headers: { "Accept": "text/html" },
          signal: controller.signal,
        });
        if (response.status === 401) { const error = new Error("401"); error.unauthorized = true; throw error; }
        if (!response.ok) throw new Error(String(response.status));
        return new DOMParser().parseFromString(await response.text(), "text/html");
      } finally {
        clearTimeout(timer);
      }
    };
    // 画面の入力が、読み込み時の値から変わっているフォームがあるか（再読み込みで入力を失わないための判定）。
    const formsChanged = () => {
      for (const field of document.querySelectorAll("form input, form textarea, form select")) {
        if (field.type === "hidden") continue;
        if (field.tagName === "SELECT") {
          if (Array.from(field.options).some(option => option.selected !== option.defaultSelected)) return true;
        } else if (field.type === "checkbox" || field.type === "radio") {
          if (field.checked !== field.defaultChecked) return true;
        } else if (field.value !== field.defaultValue) {
          return true;
        }
      }
      return false;
    };
    let wasUnauthorized = false; // 401 を見た後、まだ成功していない。
    let reloadNeeded = false; // 再読み込みしたいが、入力中のフォームがあるため注記だけを残している。
    const poll = async () => {
      if (inflight || document.hidden) return;
      inflight = true;
      try {
        applyBody(await load(false));
        if (runsPending && applyRuns(await load(true))) runsPending = false;
        if (wasUnauthorized) {
          // ログインが戻った。埋め込まれている CSRF トークンが古い可能性があるので、再読み込みする。
          // 入力中のフォームがあるときは、入力を失わないよう再読み込みせず、注記だけを残す。
          wasUnauthorized = false;
          if (formsChanged()) reloadNeeded = true; else { location.reload(); return; }
        }
        showNote(reloadNeeded ? text("reload") : "");
      } catch (error) {
        // 401 でも止めない（常駐の再起動の直後・別タブでのログインの後に、次の周期で復帰できるように）。
        if (error && error.unauthorized) wasUnauthorized = true;
        showNote(error && error.unauthorized ? text("expired") : text("stale"));
      } finally {
        inflight = false;
      }
    };
    if (initialBody) skew = Number(initialBody.dataset.now) * 1000 - Date.now();
    tick();
    setInterval(() => { if (!document.hidden && !held) tick(); }, 1000);
    setInterval(poll, POLL_MS);
    document.addEventListener("visibilitychange", () => { if (!document.hidden) { tick(); poll(); } });
  }

  // 「アプリの更新」パネル。サーバーは Accept: application/json の POST に JSON（status・message）で答える。
  // JS が無い環境では、同じフォームが HTML の結果画面を返す（インストールの form も最初から見えている）。
  function setupUpdates(panel) {
    const status = panel.querySelector("[data-update-status]");
    const checkForm = panel.querySelector("[data-update-check]");
    const installForm = panel.querySelector("[data-update-install]");
    if (!status || !checkForm || !installForm) return;
    const buttons = [checkForm, installForm].map(form => form.querySelector("button"));
    const text = name => panel.dataset[name] || "";
    const show = message => { status.textContent = message; };
    const busy = on => {
      for (const button of buttons) { button.disabled = on; button.setAttribute("aria-busy", on ? "true" : "false"); }
    };
    // フォームを fetch で送って JSON を受ける。JSON でない応答（保護の拒否ページなど）は失敗として扱う。
    const post = async (form, timeoutMs) => {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), timeoutMs);
      try {
        const response = await fetch(form.action, {
          method: "POST",
          credentials: "same-origin",
          headers: { "Accept": "application/json", "Content-Type": "application/x-www-form-urlencoded" },
          body: new URLSearchParams(new FormData(form)),
          signal: controller.signal,
        });
        // 認証済みの応答が返すプロセスの識別子（再起動の検知の基準。インストールの応答の値を使う）。
        instance = response.headers.get("x-scriptura-instance") || instance;
        const type = response.headers.get("content-type") || "";
        if (!response.ok || !type.includes("application/json")) throw new Error(String(response.status));
        return await response.json();
      } finally {
        clearTimeout(timer);
      }
    };
    // 更新を適用して再起動すると、常駐のログインが保存されていても（Cookie は 401 にならない）新しいプロセスになる。
    // 再起動の検知は、次のどれか: 401（保存されないログイン）、サーバーが一度つながらなくなった後に 200 で応答した、
    // 認証済みの応答のプロセス識別子（`x-scriptura-instance`）がインストールの応答のときから変わった。検知したら、
    // サーバーが返した引き継ぎ鍵（この変数だけに持ち、保存しない）で /auth へ入り直す。旧サーバーがまだ同じ識別子で
    // 200 を返している間は鍵を使わない。鍵が無い、または最大 180 秒待っても再起動を検知できないときは、
    // 「トレイ（メニューバー）のアイコンから開き直してください」へ切り替える。
    let instance = "";
    const waitForRestart = async handoffKey => {
      let wasDown = false;
      const baseline = instance;
      for (let i = 0; i < 90; i += 1) {
        await new Promise(resolve => setTimeout(resolve, 2000));
        try {
          const response = await fetch("/", { cache: "no-store", credentials: "same-origin" });
          const current = response.headers.get("x-scriptura-instance") || "";
          const replaced = response.status === 200 && baseline !== "" && current !== "" && current !== baseline;
          if (response.status === 401 || (response.status === 200 && wasDown) || replaced) {
            if (handoffKey) {
              location.replace(`/auth?key=${encodeURIComponent(handoffKey)}&next=updated`);
            } else {
              show(text("msgDone"));
            }
            return;
          }
        } catch (_) { wasDown = true; /* 停止中・起動中。待ち続ける */ }
      }
      show(text("msgDone"));
    };
    installForm.hidden = true;
    const check = async () => {
      busy(true);
      show(text("msgChecking"));
      try {
        const data = await post(checkForm, 45000);
        show(data.message);
        installForm.hidden = !(data.available && data.supported);
      } catch (_) {
        show(text("msgFailed"));
      } finally {
        busy(false);
      }
    };
    checkForm.addEventListener("submit", event => { event.preventDefault(); check(); });
    // ブラウザー標準の確認ではなく、画面の中のダイアログで確認する（フォーカスは「やめる」に置く）。
    const askConfirmation = message => new Promise(resolve => {
      const previousFocus = document.activeElement;
      const dialog = document.createElement("dialog");
      dialog.className = "confirmation-dialog";
      dialog.setAttribute("aria-labelledby", "confirmation-title");
      dialog.setAttribute("aria-describedby", "confirmation-message");
      const title = document.createElement("h2");
      title.id = "confirmation-title";
      title.textContent = text("confirmTitle");
      const body = document.createElement("p");
      body.id = "confirmation-message";
      body.textContent = message;
      const actions = document.createElement("div");
      actions.className = "confirmation-actions";
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.textContent = text("confirmCancel");
      cancel.autofocus = true;
      const accept = document.createElement("button");
      accept.type = "button";
      accept.className = "primary";
      accept.textContent = text("confirmOk");
      actions.append(cancel, accept);
      dialog.append(title, body, actions);
      document.body.append(dialog);
      cancel.addEventListener("click", () => dialog.close("cancel"));
      accept.addEventListener("click", () => dialog.close("confirm"));
      dialog.addEventListener("cancel", e => { e.preventDefault(); dialog.close("cancel"); });
      dialog.addEventListener("close", () => {
        const accepted = dialog.returnValue === "confirm";
        dialog.remove();
        if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
        resolve(accepted);
      }, { once: true });
      dialog.showModal();
    });
    installForm.addEventListener("submit", async event => {
      event.preventDefault();
      if (!await askConfirmation(text("confirm"))) return;
      busy(true);
      show(text("msgInstalling"));
      try {
        const data = await post(installForm, 600000);
        show(data.message);
        if (data.status === "applying") {
          installForm.hidden = true;
          if (data.restarts) { show(text("msgWaiting")); await waitForRestart(typeof data.handoff_key === "string" ? data.handoff_key : ""); }
          return;
        }
      } catch (_) {
        show(text("msgFailed"));
      }
      busy(false);
    });
    // トレイの「更新を確認」から開かれたときは、自動で確認を始める。
    if (new URLSearchParams(window.location.search).get("check_updates") === "1") check();
  }
})();
