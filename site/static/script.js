// OpenRAGE website: theme toggle, lightbox, OS-aware downloads and tabs, latest-release links.
// Everything works without JavaScript; this only adds conveniences.

(function () {
  "use strict";

  var root = document.documentElement;
  var KEY = "openrage-theme";

  function systemDark() {
    return window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  }

  function currentDark() {
    var t = root.getAttribute("data-theme");
    return t ? t === "dark" : systemDark();
  }

  function updateToggle(btn) {
    var dark = currentDark();
    btn.setAttribute("aria-pressed", dark ? "true" : "false");
    btn.setAttribute("aria-label", dark ? "Switch to light mode" : "Switch to dark mode");
    btn.title = btn.getAttribute("aria-label");
  }

  var toggle = document.querySelector(".theme-toggle");
  if (toggle) {
    toggle.hidden = false;
    updateToggle(toggle);
    toggle.addEventListener("click", function () {
      var next = currentDark() ? "light" : "dark";
      root.setAttribute("data-theme", next);
      try { localStorage.setItem(KEY, next); } catch (e) { /* storage unavailable */ }
      updateToggle(toggle);
    });
  }

  // Screenshots: click to enlarge in a dialog. Without JS the link opens the image.
  var zooms = document.querySelectorAll("a.zoom");
  if (zooms.length && window.HTMLDialogElement) {
    var box = document.createElement("dialog");
    box.className = "lightbox";
    box.setAttribute("aria-label", "Enlarged screenshot");
    box.innerHTML =
      '<button type="button" class="lightbox-close" aria-label="Close">' +
      '<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>' +
      "</button><figure><img alt=\"\"><figcaption></figcaption></figure>";
    document.body.appendChild(box);
    var boxImg = box.querySelector("img");
    var boxCaption = box.querySelector("figcaption");

    box.addEventListener("click", function (e) {
      // Clicks on the backdrop, the image or the close button close it.
      if (e.target === box || e.target === boxImg || e.target.closest(".lightbox-close")) box.close();
    });

    zooms.forEach(function (link) {
      link.setAttribute("aria-haspopup", "dialog");
      link.addEventListener("click", function (e) {
        if (e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey) return;
        var imgs = link.querySelectorAll("img");
        var img = imgs[0];
        for (var i = 0; i < imgs.length; i++) {
          if (imgs[i].offsetWidth > 0) { img = imgs[i]; break; }
        }
        e.preventDefault();
        boxImg.src = img.currentSrc || img.src;
        boxImg.alt = img.alt;
        var fig = link.closest("figure");
        var cap = fig && fig.querySelector("figcaption");
        if (cap) boxCaption.innerHTML = cap.innerHTML;
        else boxCaption.textContent = link.getAttribute("data-caption") || "";
        boxCaption.hidden = !boxCaption.textContent;
        box.showModal();
      });
    });
  }

  // The visitor's operating system: shown first in downloads and guides.
  var OS_KEY = "openrage-os";
  var OS_NAMES = { linux: "Linux", windows: "Windows" };
  function detectOS() {
    var p = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
    var ua = navigator.userAgent || "";
    if (/win/i.test(p) || /Windows/.test(ua)) return "windows";
    if (/android/i.test(ua)) return null;
    if (/linux/i.test(p) || /Linux|X11|CrOS/.test(ua)) return "linux";
    return null;
  }
  var userOS = detectOS();

  // Downloads: the visitor's system first, marked, and the primary button.
  if (userOS) {
    document.querySelectorAll('[data-os="' + userOS + '"]').forEach(function (el) {
      if (el.closest(".os-panel")) return;
      var parent = el.parentNode;
      if (parent.querySelector("[data-os]") !== el) parent.insertBefore(el, parent.firstElementChild);
      var badge = el.querySelector(".os-badge");
      if (badge) badge.hidden = false;
    });
    document.querySelectorAll(".os-buttons a[data-os]").forEach(function (a) {
      var mine = a.getAttribute("data-os") === userOS;
      a.classList.toggle("btn-primary", mine);
      a.classList.toggle("btn-secondary", !mine);
    });
    var heroLabel = document.querySelector("[data-os-download] [data-os-label]");
    if (heroLabel) heroLabel.textContent = "Download for " + OS_NAMES[userOS];
  }

  // Linux / Windows tabs. Without JS both parts stay visible with their headings.
  var tabGroups = document.querySelectorAll(".os-tabs");
  var tabSets = [];
  function selectOS(os, store) {
    tabSets.forEach(function (set) { set(os); });
    if (store) { try { localStorage.setItem(OS_KEY, os); } catch (e) { /* storage unavailable */ } }
  }
  tabGroups.forEach(function (group, g) {
    var panels = Array.prototype.filter.call(group.children, function (el) {
      return el.classList.contains("os-panel");
    });
    if (panels.length < 2) return;
    var list = document.createElement("div");
    list.className = "os-tablist";
    list.setAttribute("role", "tablist");
    list.setAttribute("aria-label", "Operating system");
    var tabs = panels.map(function (panel, i) {
      var os = panel.getAttribute("data-os");
      var id = "os-" + g + "-" + os;
      panel.id = panel.id || id + "-panel";
      panel.setAttribute("role", "tabpanel");
      panel.setAttribute("aria-labelledby", id);
      panel.setAttribute("tabindex", "0");
      var heading = panel.querySelector("h2, h3, h4");
      if (heading) heading.classList.add("visually-hidden");
      var tab = document.createElement("button");
      tab.type = "button";
      tab.id = id;
      tab.className = "os-tab";
      tab.setAttribute("role", "tab");
      tab.setAttribute("aria-controls", panel.id);
      tab.textContent = OS_NAMES[os] || os;
      if (os === userOS) {
        var mark = document.createElement("span");
        mark.className = "os-badge";
        mark.textContent = "Your system";
        tab.appendChild(document.createTextNode(" "));
        tab.appendChild(mark);
      }
      tab.addEventListener("click", function () { selectOS(os, true); });
      tab.addEventListener("keydown", function (e) {
        var step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
        if (!step) return;
        e.preventDefault();
        var next = tabs[(i + step + tabs.length) % tabs.length];
        next.click();
        next.focus();
      });
      list.appendChild(tab);
      return tab;
    });
    group.insertBefore(list, group.firstChild);
    group.classList.add("tabbed");
    tabSets.push(function (os) {
      var found = panels.some(function (p) { return p.getAttribute("data-os") === os; });
      panels.forEach(function (panel, i) {
        var on = found ? panel.getAttribute("data-os") === os : i === 0;
        panel.hidden = !on;
        tabs[i].setAttribute("aria-selected", on ? "true" : "false");
        tabs[i].tabIndex = on ? 0 : -1;
      });
    });
  });
  if (tabSets.length) {
    var stored = null;
    try { stored = localStorage.getItem(OS_KEY); } catch (e) { /* storage unavailable */ }
    selectOS(stored || userOS || "linux", false);
  }

  // Fade sections in as they scroll into view (skipped for reduced motion).
  var reveals = document.querySelectorAll(".reveal");
  var calm = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (reveals.length && "IntersectionObserver" in window && !calm) {
    root.classList.add("animate");
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          var el = entry.target;
          el.classList.add("visible");
          io.unobserve(el);
          // Hover effects should not wait for the stagger delay.
          setTimeout(function () { el.style.transitionDelay = ""; }, 900);
        }
      });
    }, { rootMargin: "0px 0px -8% 0px" });
    reveals.forEach(function (el, i) {
      // Stagger neighbours a little.
      el.style.transitionDelay = (i % 4) * 60 + "ms";
      io.observe(el);
    });
  }

  // Latest release: fill in the version and direct download links.
  // Links fall back to the releases page if the request fails.
  var targets = document.querySelectorAll("[data-release]");
  if (!targets.length || !window.fetch) return;

  fetch("https://api.github.com/repos/Two-Play/openrage/releases/latest", {
    headers: { Accept: "application/vnd.github+json" }
  })
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (rel) {
      if (!rel || !rel.tag_name) return;
      var assets = rel.assets || [];
      function find(re) {
        for (var i = 0; i < assets.length; i++) {
          if (re.test(assets[i].name)) return assets[i];
        }
        return null;
      }
      var byKind = {
        linux: find(/linux.*\.tar\.gz$/i),
        windows: find(/windows.*\.zip$/i),
        sums: find(/SHA256SUMS/i)
      };
      targets.forEach(function (el) {
        var kind = el.getAttribute("data-release");
        if (kind === "version") {
          el.textContent = rel.tag_name;
        } else if (kind === "published" && rel.published_at) {
          el.textContent = new Date(rel.published_at).toLocaleDateString("en", {
            year: "numeric", month: "long", day: "numeric"
          });
        } else if (byKind[kind]) {
          el.href = byKind[kind].browser_download_url;
          var size = el.querySelector("[data-size]");
          if (size) size.textContent = " (" + (byKind[kind].size / 1048576).toFixed(0) + " MB)";
        }
      });
      var hero = document.querySelector("[data-os-download]");
      if (hero && userOS && byKind[userOS]) hero.href = byKind[userOS].browser_download_url;
      document.querySelectorAll("[data-release-info]").forEach(function (el) {
        el.hidden = false;
      });
    })
    .catch(function () { /* keep fallback links */ });
})();
