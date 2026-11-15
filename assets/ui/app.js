"use strict";
// Scriptura 管理画面の少量の JS（CSP で同一オリジンのファイルだけを許可。インライン script は使わない）。
// 役割は文字数カウンターと言語の選択欄の自動送信だけ。送信内容の検査はすべてサーバー側で行う（ここでの表示は目安）。
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
    // 言語の選択欄は選んだ時点で送信する（JS が無い環境では「切り替える」ボタンで送る）。
    for (const select of document.querySelectorAll("select[data-autosubmit]")) {
      const form = select.closest("form");
      const fallback = form && form.querySelector("[data-autosubmit-fallback]");
      if (fallback) fallback.hidden = true;
      select.addEventListener("change", () => form && form.submit());
    }
  });
})();
