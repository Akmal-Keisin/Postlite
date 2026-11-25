/* postlite site behaviour: theme, mobile nav, scrollspy, copy buttons */

(() => {
  "use strict";

  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

  /* ----- theme (dark by default, follows the OS on first visit) ----- */

  const THEME_KEY = "postlite.site.theme";
  const root = document.documentElement;

  const stored = (() => { try { return localStorage.getItem(THEME_KEY); } catch { return null; } })();
  root.dataset.theme = stored === "light" || stored === "dark"
    ? stored
    : (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark");

  $$(".theme-toggle").forEach((btn) => {
    btn.addEventListener("click", () => {
      const next = root.dataset.theme === "light" ? "dark" : "light";
      root.dataset.theme = next;
      try { localStorage.setItem(THEME_KEY, next); } catch { /* private mode */ }
    });
  });

  /* ----- mobile nav ----- */

  const burger = $(".nav-burger");
  const links = $(".nav-links");
  if (burger && links) {
    burger.addEventListener("click", () => {
      const open = links.classList.toggle("open");
      burger.setAttribute("aria-expanded", String(open));
    });
    links.addEventListener("click", (event) => {
      if (event.target.closest("a")) {
        links.classList.remove("open");
        burger.setAttribute("aria-expanded", "false");
      }
    });
  }

  /* ----- code copy buttons ----- */

  $$(".code-block").forEach((block) => {
    const pre = block.querySelector("pre");
    const btn = block.querySelector(".code-copy");
    if (!pre || !btn) return;
    btn.addEventListener("click", async () => {
      const text = pre.textContent.replace(/^\$\s+/gm, "").trimEnd();
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        const range = document.createRange();
        range.selectNodeContents(pre);
        const sel = getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
        document.execCommand("copy");
        sel.removeAllRanges();
      }
      btn.classList.add("copied");
      const label = btn.querySelector("span");
      const old = label ? label.textContent : null;
      if (label) label.textContent = "Copied";
      btn.setAttribute("aria-label", "Copied to clipboard");
      setTimeout(() => {
        btn.classList.remove("copied");
        if (label && old) label.textContent = old;
        btn.removeAttribute("aria-label");
      }, 1800);
    });
  });

  /* ----- docs sidebar: toggle + scrollspy ----- */

  const sidebar = $(".docs-sidebar");
  const sidebarToggle = $(".docs-sidebar-toggle");
  if (sidebar && sidebarToggle) {
    sidebarToggle.addEventListener("click", () => {
      const open = sidebar.classList.toggle("open");
      sidebarToggle.setAttribute("aria-expanded", String(open));
    });
  }

  const tocLinks = $$(".docs-sidebar a[href^='#']");
  if (tocLinks.length && "IntersectionObserver" in window) {
    const sections = tocLinks
      .map((a) => document.getElementById(a.getAttribute("href").slice(1)))
      .filter(Boolean);

    const setActive = (id) => {
      tocLinks.forEach((a) => {
        const on = a.getAttribute("href") === `#${id}`;
        a.classList.toggle("active", on);
        if (on) a.setAttribute("aria-current", "true");
        else a.removeAttribute("aria-current");
      });
    };

    let current = "";
    const observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) current = entry.target.id;
      }
      if (current) setActive(current);
    }, { rootMargin: "-15% 0px -70% 0px", threshold: 0 });

    sections.forEach((sec) => observer.observe(sec));
    setActive(sections[0]?.id ?? "");
  }
})();
