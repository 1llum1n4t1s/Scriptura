"use strict";
// Scriptura 管理画面の少量の JS（CSP で同一オリジンのファイルだけを許可。インライン script は使わない）。
// 役割は文字数カウンター・選択欄とチェックボックスの自動送信・概要画面の「アプリの更新」パネルの操作だけ。送信内容の検査はすべてサーバー側で行う（ここでの表示は目安）。
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
  });

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
        const type = response.headers.get("content-type") || "";
        if (!response.ok || !type.includes("application/json")) throw new Error(String(response.status));
        return await response.json();
      } finally {
        clearTimeout(timer);
      }
    };
    // 更新を適用して再起動すると、セッションの秘密が新しくなり古い Cookie は 401 になる。自動で入り直さず、
    // 401 を見たら「トレイ（メニューバー）のアイコンから開き直してください」へ切り替える（最大 180 秒）。
    const waitForRestart = async () => {
      for (let i = 0; i < 90; i += 1) {
        await new Promise(resolve => setTimeout(resolve, 2000));
        try {
          const response = await fetch("/", { cache: "no-store", credentials: "same-origin" });
          if (response.status === 401) { show(text("msgDone")); return; }
        } catch (_) { /* 停止中・起動中。待ち続ける */ }
      }
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
    installForm.addEventListener("submit", async event => {
      event.preventDefault();
      if (!window.confirm(text("confirm"))) return;
      busy(true);
      show(text("msgInstalling"));
      try {
        const data = await post(installForm, 600000);
        show(data.message);
        if (data.status === "applying") {
          installForm.hidden = true;
          if (data.restarts) { show(text("msgWaiting")); await waitForRestart(); }
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
