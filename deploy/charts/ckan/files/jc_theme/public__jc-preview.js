// The live preview of an API resource (T-4333): the first rows of its address, fetched by the
// reader's browser without credentials and written as text only, never as markup. Without
// script, or when the platform does not answer, the page says so and the address still works.
(function () {
  var box = document.querySelector("[data-jc-preview]");
  if (!box || !window.fetch) {
    return;
  }
  var status = box.querySelector(".jc-rows-status");
  var holder = box.querySelector(".jc-rows-table");
  var kind = box.getAttribute("data-jc-kind");

  function say(text) {
    status.textContent = text;
  }

  // NGSI-LD answers an array, OData and SensorThings `value`, OGC API a FeatureCollection.
  function rowsOf(body) {
    if (kind === "NGSI-LD") {
      return Array.isArray(body) ? body : [];
    }
    if (kind === "NGSI-LD-TYPES") {
      // The API's own list of the entity types it holds, one row each.
      var label = box.getAttribute("data-jc-type-label") || "type";
      return (body && Array.isArray(body.typeList) ? body.typeList : []).map(function (name) {
        var row = {};
        row[label] = name;
        return row;
      });
    }
    if (kind === "OGCFEAT") {
      return (body && body.features ? body.features : []).map(function (f) {
        return Object.assign({ id: f.id }, f.properties || {});
      });
    }
    return body && Array.isArray(body.value) ? body.value : [];
  }

  // Anything that is not text or a number as compact JSON, cut at 80 characters.
  function cell(key, value) {
    if (value === null || value === undefined) {
      return "";
    }
    var text = typeof value === "object" ? JSON.stringify(value) : String(value);
    return text.length > 80 ? text.slice(0, 79) + "…" : text;
  }

  fetch(box.getAttribute("data-jc-preview"), {
    credentials: "omit",
    headers: { Accept: "application/json" },
  })
    .then(function (answer) {
      if (!answer.ok) {
        throw new Error(String(answer.status));
      }
      var total = answer.headers.get("NGSILD-Results-Count");
      return answer.json().then(function (body) {
        var rows = rowsOf(body);
        // A types list is shown whole (a handful of names); entities five at a time.
        return { rows: kind === "NGSI-LD-TYPES" ? rows : rows.slice(0, 5), total: total };
      });
    })
    .then(function (result) {
      var rows = result.rows;
      if (!rows.length) {
        say(box.getAttribute("data-jc-empty"));
        return;
      }
      // The values a reader came for first: provenance is the same on every row of a dataset.
      var skip = { "@context": 1, type: 1, "@odata.etag": 1, id: 1, dataset: 1, dataProvider: 1, source: 1 };
      var keys = [];
      rows.forEach(function (row) {
        Object.keys(row).forEach(function (key) {
          if (!skip[key] && keys.indexOf(key) < 0) {
            keys.push(key);
          }
        });
      });
      keys = keys.slice(0, 7);
      var table = document.createElement("table");
      table.className = "table table-sm";
      var head = table.createTHead().insertRow();
      keys.forEach(function (key) {
        var th = document.createElement("th");
        th.scope = "col";
        th.textContent = key;
        head.appendChild(th);
      });
      var body = table.createTBody();
      rows.forEach(function (row) {
        var tr = body.insertRow();
        keys.forEach(function (key) {
          tr.insertCell().textContent = cell(key, row[key]);
        });
      });
      holder.appendChild(table);
      holder.hidden = false;
      var total = result.total && /^\d+$/.test(result.total) ? Number(result.total) : null;
      if (total !== null) {
        var count = document.querySelector("[data-jc-count]");
        if (count) {
          count.querySelector("dd").textContent = total.toLocaleString(document.documentElement.lang || undefined);
          count.hidden = false;
        }
        say(box.getAttribute("data-jc-shown").replace("{shown}", rows.length).replace("{total}", total.toLocaleString(document.documentElement.lang || undefined)));
      } else {
        status.textContent = "";
        status.hidden = true;
      }
    })
    .catch(function () {
      say(box.getAttribute("data-jc-failed"));
    });
})();
