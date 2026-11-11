// OpenRAGE website: theme toggle and latest-release links.
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
      document.querySelectorAll("[data-release-info]").forEach(function (el) {
        el.hidden = false;
      });
    })
    .catch(function () { /* keep fallback links */ });
})();
