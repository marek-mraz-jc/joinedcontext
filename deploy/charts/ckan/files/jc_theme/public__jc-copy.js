// The copy buttons of every page (T-2744, T-4762), loaded once by base.html. Without script the
// text is a read-only field or block a person selects and copies by hand, so nothing depends on
// this file. A button copies `data-jc-text` when it has one (the one-line command or plain JSON
// behind a block laid out for a narrow card), else the text of the element it names.
document.addEventListener("click", function (event) {
  var button = event.target.closest("[data-jc-copy]");
  var field = button && document.getElementById(button.getAttribute("data-jc-copy"));
  if (!field) {
    return;
  }
  var text = button.getAttribute("data-jc-text");
  if (text === null) {
    text = field.value !== undefined ? field.value : field.textContent;
  }
  var say = function (words, ms) {
    var status = document.getElementById("jc-copy-status");
    if (status) {
      status.textContent = words;
    }
    // A button with its own label says so where the eye is, then goes back (T-4284).
    var label = button.querySelector(".jc-code-copy-label");
    if (label && words) {
      var was = label.getAttribute("data-label") || label.textContent;
      label.setAttribute("data-label", was);
      label.textContent = words;
      clearTimeout(label.jcTimer);
      label.jcTimer = setTimeout(function () {
        label.textContent = was;
      }, ms);
    }
  };
  var copied = function () {
    say(button.getAttribute("data-jc-copied"), 2000);
  };
  // No Clipboard API (an http origin) or a refusal (a frame without clipboard-write, a policy):
  // the browser's own copy command, else the text stays selected for Ctrl+C and the button says so.
  var fallback = function () {
    var area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    var done = false;
    try {
      done = document.execCommand("copy");
    } catch (error) {
      done = false;
    }
    area.remove();
    if (done) {
      copied();
      return;
    }
    if (field.select) {
      field.select();
    } else {
      var range = document.createRange();
      range.selectNodeContents(field);
      var selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
    }
    say(button.getAttribute("data-jc-select"), 6000);
  };
  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(text).then(copied, fallback);
  } else {
    fallback();
  }
});
