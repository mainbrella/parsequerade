const menuToggle = document.querySelector(".menu-toggle");
const navigation = document.querySelector("#main-navigation");
function closeMenu() {
  menuToggle.setAttribute("aria-expanded", "false");
  menuToggle.setAttribute("aria-label", "Open navigation");
  navigation.dataset.open = "false";
}
menuToggle.addEventListener("click", () => {
  const open = menuToggle.getAttribute("aria-expanded") !== "true";
  menuToggle.setAttribute("aria-expanded", String(open));
  menuToggle.setAttribute("aria-label", open ? "Close navigation" : "Open navigation");
  navigation.dataset.open = String(open);
});
navigation.addEventListener("click", (event) => {
  if (event.target instanceof Element && event.target.closest("a")) closeMenu();
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && menuToggle.getAttribute("aria-expanded") === "true") {
    closeMenu();
    menuToggle.focus();
  }
});
document.addEventListener("click", (event) => {
  if (event.target instanceof Node && !navigation.contains(event.target) && !menuToggle.contains(event.target)) closeMenu();
});
window.matchMedia("(min-width: 801px)").addEventListener("change", closeMenu);
function renderCode(container, source) {
  const lines = source.split("\n").map((line) => {
    const row = document.createElement("span");
    row.className = "code-line";
    const tokens = /"(?:\\.|[^"\\])*"|'[^']*'|\bcurl\b/g;
    let previous = 0;
    for (const match of line.matchAll(tokens)) {
      row.append(document.createTextNode(line.slice(previous, match.index)));
      const token = document.createElement("span");
      token.className = match[0] === "curl" ? "command" : /^\s*:/.test(line.slice(match.index + match[0].length)) ? "key" : "string";
      token.textContent = match[0];
      row.append(token);
      previous = match.index + match[0].length;
    }
    row.append(document.createTextNode(line.slice(previous)));
    return row;
  });
  container.querySelector("code").replaceChildren(...lines);
}
const localizationResponse = Array.from(document.querySelectorAll("#result-response .code-line"), (line) => line.textContent).join("\n");
const listResponse = Array.from(document.querySelectorAll(".example-response .code-line"), (line) => line.textContent).join("\n");
const localizationRequest = Array.from(document.querySelectorAll("#localization-request .code-line"), (line) => line.textContent).join("\n");
const shell = 'curl https://api.parsequerade.com/apps?bundleId=com.example.notes \\\n  -H "Authorization: Bearer YOUR_API_KEY"';
const requestExamples = {
  shell,
  javascript: 'const response = await fetch(\n  "https://api.parsequerade.com/apps?bundleId=com.example.notes",\n  { headers: { Authorization: "Bearer YOUR_API_KEY" } }\n);\nconst { data } = await response.json();',
  python: 'import requests\n\nresponse = requests.get(\n    "https://api.parsequerade.com/apps",\n    params={"bundleId": "com.example.notes"},\n    headers={"Authorization": "Bearer YOUR_API_KEY"},\n)\napps = response.json()["data"]'
};
renderCode(document.querySelector("#localization-response"), localizationResponse);
renderCode(document.querySelector("#localization-curl"), `curl -X PATCH \\
  https://api.parsequerade.com/apps/123456789/versions/987654321/localizations/en-US \\
  -H "Authorization: Bearer YOUR_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '` + localizationRequest + "'");
renderCode(document.querySelector("#example-response"), listResponse);
renderCode(document.querySelector("#example-curl"), shell);
const language = document.querySelector("#code-language");
const copyButton = document.querySelector(".copy-button");
const copyStatus = document.querySelector(".copy-status");
let copiedTimer;
function resetCopyStatus() {
  clearTimeout(copiedTimer);
  delete copyButton.dataset.copied;
  copyButton.setAttribute("aria-label", "Copy example code");
  copyButton.querySelector("use").setAttribute("href", "#icon-copy");
  copyStatus.textContent = "";
}
document.querySelectorAll("[data-tabs]").forEach((tablist) => {
  const tabs = Array.from(tablist.querySelectorAll('[role="tab"]'));
  function selectTab(selected) {
    tabs.forEach((tab) => {
      const active = tab === selected;
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
      document.getElementById(tab.getAttribute("aria-controls")).hidden = !active;
    });
    if (tablist.dataset.tabs === "example") {
      language.disabled = selected.dataset.tab !== "request";
      resetCopyStatus();
    }
  }
  tabs.forEach((tab, index) => {
    tab.addEventListener("click", () => selectTab(tab));
    tab.addEventListener("keydown", (event) => {
      let next;
      if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
      else if (event.key === "ArrowLeft") next = (index - 1 + tabs.length) % tabs.length;
      else if (event.key === "Home") next = 0;
      else if (event.key === "End") next = tabs.length - 1;
      else return;
      event.preventDefault();
      selectTab(tabs[next]);
      tabs[next].focus();
    });
  });
});
language.addEventListener("change", () => {
  renderCode(document.querySelector("#example-request"), requestExamples[language.value]);
  resetCopyStatus();
});
copyButton.addEventListener("click", async () => {
  const active = document.querySelector('.example-request [role="tabpanel"]:not([hidden])');
  const source = Array.from(active.querySelectorAll(".code-line"), (line) => line.textContent).join("\n");
  try {
    await navigator.clipboard.writeText(source);
    copyButton.dataset.copied = "true";
    copyButton.setAttribute("aria-label", "Code copied");
    copyButton.querySelector("use").setAttribute("href", "#icon-check");
    copyStatus.textContent = "Code copied to clipboard.";
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(resetCopyStatus, 2e3);
  } catch {
    copyStatus.classList.remove("sr-only");
    copyStatus.textContent = "Could not copy. Select the code to copy it manually.";
  }
});
