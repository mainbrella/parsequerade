(() => {
  const sidebar = document.querySelector(".docs-sidebar");
  if (!sidebar) return;

  document.documentElement.classList.add("js");

  const searchInput = document.querySelector("#docs-search");
  const searchResults = document.querySelector("#docs-search-results");
  const mobileMenuButtons = [...document.querySelectorAll(".mobile-menu-toggle, .crumb-menu")];
  const searchButton = document.querySelector(".mobile-search-toggle");
  const backdrop = document.querySelector(".sidebar-backdrop");
  const sidebarClose = document.querySelector(".sidebar-close");
  const main = document.querySelector(".docs-main");
  const header = document.querySelector(".docs-header");
  let searchIndex = null;
  let searchLoad = null;
  let searchFailed = false;
  let scrollTimer;
  let lastDrawerOpener = null;

  window.addEventListener("scroll", () => {
    document.documentElement.classList.add("is-scrolling");
    window.clearTimeout(scrollTimer);
    scrollTimer = window.setTimeout(() => document.documentElement.classList.remove("is-scrolling"), 700);
  }, { passive: true });

  const isMobile = () => window.matchMedia("(max-width: 800px)").matches;
  const setSidebar = (open, opener) => {
    const mobile = isMobile();
    if (open && opener) lastDrawerOpener = opener;
    sidebar.classList.toggle("is-open", open);
    document.body.classList.toggle("no-scroll", open && mobile);
    if (main) {
      if (open && mobile) main.setAttribute("inert", "");
      else main.removeAttribute("inert");
    }
    if (header) {
      if (open && mobile) header.setAttribute("inert", "");
      else header.removeAttribute("inert");
    }
    if (mobile && open) {
      sidebar.setAttribute("role", "dialog");
      sidebar.setAttribute("aria-modal", "true");
    } else {
      sidebar.removeAttribute("role");
      sidebar.removeAttribute("aria-modal");
    }
    mobileMenuButtons.forEach((button) => button.setAttribute("aria-expanded", String(open && mobile)));
    searchButton?.setAttribute("aria-expanded", String(open && mobile));
    if (backdrop) backdrop.hidden = !(open && mobile);
  };

  const openDrawer = (opener) => {
    setSidebar(true, opener);
    if (searchInput) searchInput.focus({ preventScroll: true });
    else sidebarClose?.focus({ preventScroll: true });
  };

  mobileMenuButtons.forEach((button) => button.addEventListener("click", () => {
    const open = !sidebar.classList.contains("is-open");
    if (open && isMobile()) openDrawer(button);
    else setSidebar(open, button);
  }));

  searchButton?.addEventListener("click", () => {
    openDrawer(searchButton);
  });

  const closeDrawer = () => {
    setSidebar(false);
    (lastDrawerOpener || mobileMenuButtons[0] || searchButton)?.focus();
  };
  backdrop?.addEventListener("click", closeDrawer);
  sidebarClose?.addEventListener("click", closeDrawer);

  document.addEventListener("keydown", (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      if (isMobile()) openDrawer(searchButton || mobileMenuButtons[0]);
      searchInput?.focus();
    }
    if (event.key === "Escape" && sidebar.classList.contains("is-open") && isMobile()) {
      event.preventDefault();
      closeDrawer();
    }
    if (event.key === "Tab" && sidebar.classList.contains("is-open") && isMobile()) {
      const focusables = [...sidebar.querySelectorAll('a[href]:not([hidden]), button:not([disabled]):not([hidden]), input:not([disabled]), summary, [tabindex="0"]')]
        .filter((item) => item.offsetParent !== null);
      if (!focusables.length) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      if (event.shiftKey && (document.activeElement === first || !sidebar.contains(document.activeElement))) {
        event.preventDefault(); last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault(); first.focus();
      }
    }
  });

  window.addEventListener("resize", () => {
    if (!isMobile()) setSidebar(false);
  });

  sidebar.querySelectorAll("a[href]").forEach((link) => link.addEventListener("click", () => setSidebar(false)));

  const tokenize = (value) => value.toLocaleLowerCase().match(/[\p{L}\p{N}_./-]+/gu) || [];
  const getIndex = async () => {
    if (searchIndex) return searchIndex;
    if (searchLoad) return searchLoad;
    searchLoad = fetch("/docs/reference-index.json", { headers: { Accept: "application/json" } })
      .then((response) => {
        if (!response.ok) throw new Error("Search index unavailable");
        return response.json();
      })
      .then((entries) => {
        if (!Array.isArray(entries)) throw new Error("Invalid search index");
        searchIndex = entries;
        searchFailed = false;
        return entries;
      })
      .catch((error) => {
        searchFailed = true;
        searchLoad = null;
        throw error;
      });
    return searchLoad;
  };

  const rankSearch = (entry, terms) => {
    const title = (entry.title || "").toLocaleLowerCase();
    const route = (entry.route || "").toLocaleLowerCase();
    const fields = [title, route, entry.applePath, entry.group, entry.keywords]
      .map((value) => String(value || "").toLocaleLowerCase());
    if (!terms.every((term) => fields.some((field) => field.includes(term)))) return -1;
    return terms.reduce((score, term) => score + (title.includes(term) ? 8 : 0) + (route.includes(term) ? 6 : 0)
      + (String(entry.applePath || "").toLocaleLowerCase().includes(term) ? 3 : 0)
      + (String(entry.group || "").toLocaleLowerCase().includes(term) ? 2 : 0)
      + (String(entry.keywords || "").toLocaleLowerCase().includes(term) ? 1 : 0), 0);
  };

  const showSearchStatus = (message) => {
    if (!searchResults) return;
    searchResults.replaceChildren();
    const status = document.createElement("p");
    status.className = "search-status";
    status.textContent = message;
    searchResults.append(status);
    searchResults.hidden = false;
  };

  const renderResults = (entries, terms) => {
    if (!searchResults) return;
    searchResults.replaceChildren();
    if (!terms.length) { searchResults.hidden = true; return; }
    const matches = entries.map((entry) => ({ entry, score: rankSearch(entry, terms) }))
      .filter((result) => result.score >= 0)
      .sort((a, b) => b.score - a.score || String(a.entry.title).localeCompare(String(b.entry.title)))
      .slice(0, 20);
    const count = document.createElement("p");
    count.className = "search-status";
    count.textContent = `${matches.length}${matches.length === 20 ? "+" : ""} ${matches.length === 1 ? "result" : "results"}`;
    searchResults.append(count);
    if (!matches.length) {
      const empty = document.createElement("p");
      empty.className = "search-status";
      empty.textContent = "No matching endpoints.";
      searchResults.append(empty);
    }
    matches.forEach(({ entry }) => {
      const link = document.createElement("a");
      link.className = "search-result";
      link.href = entry.url;
      const title = document.createElement("span");
      title.className = "search-result-title";
      title.textContent = entry.title || entry.route || "Endpoint";
      const route = document.createElement("code");
      route.textContent = entry.route || "";
      link.append(title, route);
      link.addEventListener("click", () => setSidebar(false));
      searchResults.append(link);
    });
    searchResults.hidden = false;
  };

  let searchSequence = 0;
  const updateSearch = async () => {
    if (!searchInput || !searchResults) return;
    const sequence = ++searchSequence;
    const terms = tokenize(searchInput.value);
    document.querySelectorAll(".nav-group").forEach((group) => {
      const links = [...group.querySelectorAll(".nav-link")];
      let visible = 0;
      links.forEach((link) => {
        const match = !terms.length || terms.every((term) => link.textContent.toLocaleLowerCase().includes(term));
        link.hidden = !match;
        if (match) visible += 1;
      });
      if (group.tagName === "DETAILS") group.hidden = visible === 0;
      else group.hidden = visible === 0;
    });
    if (!terms.length) { searchResults.hidden = true; return; }
    showSearchStatus("Searching endpoints…");
    try {
      const entries = await getIndex();
      if (sequence === searchSequence) renderResults(entries, terms);
    } catch {
      if (sequence === searchSequence) showSearchStatus("Search is unavailable. Focus the search box to try again.");
    }
  };
  searchInput?.addEventListener("focus", () => {
    if (searchFailed) {
      searchLoad = null;
      searchFailed = false;
      if (searchInput.value.trim()) updateSearch();
      else getIndex().catch(() => {});
      return;
    }
    getIndex().catch(() => {});
  });
  searchInput?.addEventListener("input", updateSearch);

  document.querySelectorAll(".code-tabs").forEach((tablist) => {
    const tabs = [...tablist.querySelectorAll('[role="tab"]')];
    const activate = (tab, focus = false) => {
      tabs.forEach((item) => {
        const selected = item === tab;
        item.setAttribute("aria-selected", String(selected));
        item.tabIndex = selected ? 0 : -1;
        const panel = document.getElementById(item.getAttribute("aria-controls"));
        if (panel) panel.hidden = !selected;
      });
      if (focus) tab.focus();
    };
    tabs.forEach((tab, index) => {
      tab.addEventListener("click", () => activate(tab));
      tab.addEventListener("keydown", (event) => {
        let next = index;
        if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
        else if (event.key === "ArrowLeft") next = (index - 1 + tabs.length) % tabs.length;
        else if (event.key === "Home") next = 0;
        else if (event.key === "End") next = tabs.length - 1;
        else return;
        event.preventDefault(); activate(tabs[next], true);
      });
    });
    const selected = tabs.find((tab) => tab.getAttribute("aria-selected") === "true") || tabs[0];
    if (selected) activate(selected);
  });

  const getCopyText = (button) => {
    const targetId = button.dataset.copyTarget;
    const target = targetId ? document.getElementById(targetId) : null;
    if (!target) return window.location.href;
    if (target.matches(".endpoint-line")) return target.querySelector("code")?.textContent || target.textContent;
    return target.innerText || target.textContent || "";
  };
  const copyText = async (value) => {
    if (navigator.clipboard?.writeText) return navigator.clipboard.writeText(value);
    const helper = document.createElement("textarea");
    helper.value = value;
    helper.setAttribute("readonly", "");
    helper.style.position = "fixed";
    helper.style.opacity = "0";
    document.body.append(helper);
    helper.select();
    const copied = document.execCommand("copy");
    helper.remove();
    if (!copied) throw new Error("Copy failed");
  };
  document.querySelectorAll("[data-copy-target], [data-copy-page]").forEach((button) => {
    const feedback = button.querySelector(".copy-label") || button.querySelector(".copy-text");
    feedback?.setAttribute("aria-live", "polite");
    feedback?.setAttribute("aria-atomic", "true");
    button.addEventListener("click", async () => {
      const label = button.querySelector(".copy-label") || button.querySelector(".copy-text");
      const original = button.dataset.copyOriginal || label?.textContent || "Copy";
      button.dataset.copyOriginal = original;
      try {
        await copyText(button.hasAttribute("data-copy-page") && !button.dataset.copyTarget ? window.location.href : getCopyText(button));
        if (label) label.textContent = "Copied";
        button.classList.add("is-copied");
        button.setAttribute("aria-label", "Copied to clipboard");
        window.setTimeout(() => {
          if (label) label.textContent = original;
          button.classList.remove("is-copied");
          button.removeAttribute("aria-label");
        }, 1600);
      } catch {
        if (label) label.textContent = "Copy failed";
        button.setAttribute("aria-label", "Copy failed");
      }
    });
  });
})();
