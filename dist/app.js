const sidebar = document.querySelector(".docs-sidebar");
const searchInput = document.querySelector("#docs-search");
const mobileMenuButtons = document.querySelectorAll(".mobile-menu-toggle, .crumb-menu");

let scrollTimer;
window.addEventListener("scroll", () => {
  document.documentElement.classList.add("is-scrolling");
  window.clearTimeout(scrollTimer);
  scrollTimer = window.setTimeout(() => document.documentElement.classList.remove("is-scrolling"), 700);
}, { passive: true });

function setSidebar(open) {
  if (!sidebar) return;
  sidebar.classList.toggle("is-open", open);
  document.body.classList.toggle("no-scroll", open && window.innerWidth <= 800);
  mobileMenuButtons.forEach((button) => button.setAttribute("aria-expanded", String(open)));
}

mobileMenuButtons.forEach((button) => {
  button.addEventListener("click", () => setSidebar(!sidebar?.classList.contains("is-open")));
});

document.querySelector(".mobile-search-toggle")?.addEventListener("click", () => {
  setSidebar(true);
  window.setTimeout(() => searchInput?.focus(), 0);
});

document.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
    event.preventDefault();
    setSidebar(true);
    searchInput?.focus();
  }
  if (event.key === "Escape" && sidebar?.classList.contains("is-open")) {
    setSidebar(false);
    mobileMenuButtons[0]?.focus();
  }
});

searchInput?.addEventListener("input", (event) => {
  const query = event.target.value.trim().toLowerCase();
  let visibleCount = 0;
  document.querySelectorAll(".nav-group").forEach((group) => {
    let visibleLinks = 0;
    group.querySelectorAll(".nav-link").forEach((link) => {
      const matches = !query || link.textContent.toLowerCase().includes(query);
      link.hidden = !matches;
      if (matches) visibleLinks += 1;
    });
    group.hidden = visibleLinks === 0;
    visibleCount += visibleLinks;
  });
  const emptyMessage = document.querySelector(".search-empty");
  if (emptyMessage) emptyMessage.hidden = visibleCount > 0;
});

document.querySelectorAll(".docs-sidebar a[href]").forEach((link) => {
  link.addEventListener("click", () => setSidebar(false));
});

document.querySelector("[data-copy-page]")?.addEventListener("click", async (event) => {
  const button = event.currentTarget;
  try {
    await navigator.clipboard.writeText(window.location.href);
    button.classList.add("is-copied");
    window.setTimeout(() => button.classList.remove("is-copied"), 1600);
  } catch {
    button.setAttribute("aria-label", "Copy unavailable");
  }
});
