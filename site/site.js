const copyButton = document.querySelector("[data-copy]");

copyButton?.addEventListener("click", async () => {
  const text = copyButton.dataset.copy;

  if (!text || !navigator.clipboard) {
    return;
  }

  try {
    await navigator.clipboard.writeText(text);
    copyButton.textContent = "Copied";
    window.setTimeout(() => {
      copyButton.textContent = "Copy";
    }, 1600);
  } catch {
    copyButton.textContent = "Select";
  }
});
