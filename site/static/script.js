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
