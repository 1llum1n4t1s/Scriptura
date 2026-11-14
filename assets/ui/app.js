"use strict";
// Scriptura 管理画面の少量の JS（CSP で同一オリジンのファイルだけを許可。インライン script は使わない）。
// 役割は文字数カウンターだけ。送信内容の検査はすべてサーバー側で行う（ここでの表示は目安）。
(() => {
  const encoder = new TextEncoder();
  // 改行は保存時に LF へ揃えるので、数えるときも CRLF を 1 文字として扱う。
  const normalize = text => text.replace(/\r\n?/g, "\n");
  const update = (area, counter) => {
    const text = normalize(area.value);
    const maxChars = Number(area.dataset.max || 0);
    const maxBytes = Number(area.dataset.maxBytes || 0);
    if (maxChars) {
      const count = [...text].length;
      counter.textContent = `${count} / ${maxChars} 字`;
      counter.classList.toggle("over", count > maxChars);
    } else if (maxBytes) {
      const bytes = encoder.encode(text).length;
      counter.textContent = `${bytes} / ${maxBytes} バイト`;
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
  });
})();
