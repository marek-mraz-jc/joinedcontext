// What CKAN 2.11's own templates wrote as onclick attributes, which the catalogue's CSP refuses
// (T-5147): one listener for the whole page, so a button DataTables clones or draws later works too.
// - the resource form's Upload, Link, Remove and Clear Upload (resource_upload_field.html):
//   data-jc-check ticks a radio or box, data-jc-clear empties a field, then data-jc-click clicks one
//   or data-jc-focus focuses one;
// - the table view's Refit button and its record dialog's Copy and Print (datatablesview.js);
// - the table view form's _id box, which stays ticked.
(function () {
  "use strict";
  function byId(id) {
    return id ? document.getElementById(id) : null;
  }
  function tableTitle() {
    var table = byId("dtprv");
    return table ? table.dataset.packageName + "—" + table.dataset.resourceName : "";
  }
  document.addEventListener("click", function (event) {
    var target = event.target instanceof Element ? event.target : null;
    if (!target) {
      return;
    }
    var button = target.closest("[data-jc-check]");
    if (button) {
      var box = byId(button.dataset.jcCheck);
      if (box) {
        box.checked = true;
      }
      var field = byId(button.dataset.jcClear);
      if (field) {
        field.value = "";
      }
      var click = byId(button.dataset.jcClick);
      if (click) {
        click.click();
      }
      var focus = byId(button.dataset.jcFocus);
      if (focus) {
        focus.focus();
      }
      return;
    }
    if (target.closest("#refit-button") && typeof window.fitColText === "function") {
      window.fitColText();
    } else if (target.closest("#modalcopy-button") && typeof window.copyModal === "function") {
      window.copyModal(tableTitle());
    } else if (target.closest("#modalprint-button") && typeof window.printModal === "function") {
      window.printModal(tableTitle());
    } else if (target.matches("#field-show_fields_1[readonly]")) {
      event.preventDefault();
    }
  });
})();
