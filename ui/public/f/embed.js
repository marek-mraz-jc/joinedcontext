// Embeds a joinedcontext public form (API/01 §33, EP-101):
//   <script src="https://{portal}/f/embed.js" data-jc-form="{slug}" async></script>
// inserts the form's page as a frame right after this tag and sizes the frame to the form from
// the page's one message, {"type": "jc-form-height", "height": n}. The site must be named in the
// form's embedOrigins, or the browser refuses the frame.
(function () {
  "use strict";
  var script = document.currentScript;
  if (!script) return;
  var slug = script.getAttribute("data-jc-form") || "";
  if (!/^[a-z0-9-]{1,63}$/.test(slug)) {
    console.error("joinedcontext form: data-jc-form names no form slug");
    return;
  }
  var portal = new URL(script.src).origin;
  var frame = document.createElement("iframe");
  frame.src = portal + "/f/" + slug;
  frame.title = script.getAttribute("data-jc-title") || "Form";
  frame.loading = "lazy";
  frame.style.width = "100%";
  frame.style.border = "0";
  frame.style.minHeight = "320px";
  script.parentNode.insertBefore(frame, script.nextSibling);
  window.addEventListener("message", function (event) {
    // Only this frame's page, from the Portal, may size it; anything else is ignored.
    if (event.origin !== portal || event.source !== frame.contentWindow) return;
    var data = event.data;
    if (!data || data.type !== "jc-form-height" || typeof data.height !== "number") return;
    var height = Math.max(0, Math.min(Math.ceil(data.height), 20000));
    frame.style.height = height + "px";
  });
})();
