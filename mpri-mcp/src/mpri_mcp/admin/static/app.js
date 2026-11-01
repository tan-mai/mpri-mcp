// Dashboard behaviour. Loaded from /static (the CSP forbids inline scripts).
(function () {
  "use strict";

  // <form data-confirm="Are you sure?"> asks before submitting.
  document.addEventListener("submit", function (event) {
    var form = event.target;
    var message = form.getAttribute && form.getAttribute("data-confirm");
    if (message && !window.confirm(message)) event.preventDefault();
  });

  function setNav(open) {
    document.body.classList.toggle("nav-open", open);
    var toggle = document.querySelector("[data-nav-toggle]");
    if (toggle) toggle.setAttribute("aria-expanded", String(open));
  }

  function copyText(button) {
    var text = button.getAttribute("data-copy");
    var label = button.querySelector("span");
    var done = function () {
      if (!label) return;
      var before = label.textContent;
      label.textContent = "Copied";
      setTimeout(function () { label.textContent = before; }, 1500);
    };
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(text).then(done);
    } else {
      var area = document.createElement("textarea");
      area.value = text;
      document.body.appendChild(area);
      area.select();
      document.execCommand("copy");
      area.remove();
      done();
    }
  }

  // Delegated clicks: dialogs, mobile nav, copy buttons, dismissable alerts.
  document.addEventListener("click", function (event) {
    var el = event.target.closest ? event.target : null;
    if (!el) return;
    var opener = el.closest("[data-dialog-open]");
    if (opener) {
      var dialog = document.getElementById(opener.getAttribute("data-dialog-open"));
      var title = opener.getAttribute("data-dialog-title");
      if (title && dialog) dialog.querySelector(".modal-head h2").textContent = title;
      if (dialog && !dialog.open) dialog.showModal();
      return;
    }
    if (el.closest("[data-dialog-close]")) {
      var open = el.closest("dialog");
      if (open) open.close();
      return;
    }
    // A click on the backdrop lands on the <dialog> itself.
    if (el.tagName === "DIALOG" && el.open) {
      var box = el.getBoundingClientRect();
      var inside = event.clientX >= box.left && event.clientX <= box.right &&
        event.clientY >= box.top && event.clientY <= box.bottom;
      if (!inside) el.close();
      return;
    }
    if (el.closest("[data-nav-toggle]")) return setNav(!document.body.classList.contains("nav-open"));
    if (el.closest("[data-nav-close]")) return setNav(false);
    var copy = el.closest("[data-copy]");
    if (copy) return copyText(copy);
    var dismiss = el.closest("[data-dismiss]");
    if (dismiss) dismiss.parentElement.remove();
  });

  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape") setNav(false);
  });

  // <select data-autosubmit> applies a filter as soon as it changes.
  // <input type="checkbox" data-select-all="name"> ticks every enabled box of that name in its form.
  document.addEventListener("change", function (event) {
    var target = event.target;
    if (!target.hasAttribute) return;
    if (target.hasAttribute("data-autosubmit")) {
      target.form.submit();
      return;
    }
    var form = target.form;
    if (!form) return;
    var group = target.getAttribute("data-select-all");
    if (group) {
      form.querySelectorAll('input[name="' + group + '"]:not(:disabled)').forEach(function (box) {
        box.checked = target.checked;
      });
      return;
    }
    var all = form.querySelector('[data-select-all="' + target.name + '"]');
    if (all) {
      var boxes = form.querySelectorAll('input[name="' + target.name + '"]:not(:disabled)');
      all.checked = Array.prototype.every.call(boxes, function (box) { return box.checked; });
    }
  });

  // A form that failed validation re-renders with its dialog open.
  document.querySelectorAll("dialog[data-open]").forEach(function (d) { d.showModal(); });

  var root = document.getElementById("price-chart");
  if (!root || !window.Chart) return;
  window.Chart.defaults.font.family = getComputedStyle(document.body).fontFamily;

  var MAX_SERIES = 8; // a 9th series never gets a generated hue
  var canvas = root.querySelector("canvas");
  var emptyBox = root.querySelector(".chart-empty");
  var tableBox = document.getElementById("price-table");
  var src = root.getAttribute("data-src");
  var chart = null;
  var lastData = null;

  function token(name) {
    return getComputedStyle(root).getPropertyValue(name).trim();
  }
  function vnd(n) {
    return n === null || n === undefined ? "—" : Number(n).toLocaleString("vi-VN") + " ₫";
  }
  function compactVnd(n) {
    return n >= 1000 ? Math.round(n / 1000).toLocaleString("vi-VN") + "k" : String(n);
  }

  // Vertical hairline at the hovered date: readers aim at a date, not at a 2px line.
  var crosshair = {
    id: "crosshair",
    afterDatasetsDraw: function (c) {
      var active = c.tooltip && c.tooltip.getActiveElements();
      if (!active || !active.length) return;
      var x = active[0].element.x;
      var ctx = c.ctx;
      ctx.save();
      ctx.strokeStyle = token("--chart-axis");
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(x, c.chartArea.top);
      ctx.lineTo(x, c.chartArea.bottom);
      ctx.stroke();
      ctx.restore();
    },
  };

  // Supermarket name at each line's last point (2–4 series), in text ink, nudged apart.
  var endLabels = {
    id: "endLabels",
    afterDatasetsDraw: function (c) {
      var n = c.data.datasets.length;
      if (n < 2 || n > 4 || c.width < 520) return;
      var labels = [];
      c.data.datasets.forEach(function (ds, i) {
        var meta = c.getDatasetMeta(i);
        if (meta.hidden) return;
        for (var j = ds.data.length - 1; j >= 0; j--) {
          if (ds.data[j].y !== null) {
            labels.push({ text: ds.label, x: meta.data[j].x, y: meta.data[j].y });
            break;
          }
        }
      });
      labels.sort(function (a, b) { return a.y - b.y; });
      for (var k = 1; k < labels.length; k++) {
        if (labels[k].y - labels[k - 1].y < 14) labels[k].y = labels[k - 1].y + 14;
      }
      var ctx = c.ctx;
      ctx.save();
      ctx.font = "12px system-ui, -apple-system, 'Segoe UI', sans-serif";
      ctx.fillStyle = token("--chart-text-secondary");
      ctx.textBaseline = "middle";
      labels.forEach(function (l) { ctx.fillText(l.text, l.x + 8, l.y); });
      ctx.restore();
    },
  };

  function lastIndex(values) {
    for (var i = values.length - 1; i >= 0; i--) if (values[i].y !== null) return i;
    return -1;
  }

  function buildTable(series, dates) {
    tableBox.textContent = "";
    var table = document.createElement("table");
    var head = table.createTHead().insertRow();
    ["Date"].concat(series.map(function (s) { return s.name; })).forEach(function (h) {
      var th = document.createElement("th");
      th.textContent = h;
      head.appendChild(th);
    });
    var body = table.createTBody();
    dates.slice().reverse().forEach(function (d) {
      var row = body.insertRow();
      row.insertCell().textContent = d;
      series.forEach(function (s) {
        var p = s.points.find(function (pt) { return pt.date === d; });
        var cell = row.insertCell();
        cell.className = "num";
        cell.textContent = p ? vnd(p.current) : "";
      });
    });
    var wrap = document.createElement("div");
    wrap.className = "table-wrap";
    wrap.appendChild(table);
    tableBox.appendChild(wrap);
  }

  function render(data) {
    lastData = data;
    var series = data.series.slice(0, MAX_SERIES);
    var dateSet = {};
    series.forEach(function (s) { s.points.forEach(function (p) { dateSet[p.date] = true; }); });
    var dates = Object.keys(dateSet).sort();
    if (chart) { chart.destroy(); chart = null; }
    var empty = dates.length === 0;
    emptyBox.hidden = !empty;
    canvas.hidden = empty;
    buildTable(series, dates);
    if (empty) return;

    var surface = token("--chart-surface");
    var datasets = series.map(function (s, i) {
      var color = token("--series-" + (i + 1));
      var byDate = {};
      s.points.forEach(function (p) { byDate[p.date] = p; });
      var values = dates.map(function (d) {
        var p = byDate[d];
        return p
          ? { x: d, y: p.current, original: p.original, member: p.member }
          : { x: d, y: null };
      });
      var last = lastIndex(values);
      var few = values.filter(function (v) { return v.y !== null; }).length <= 10;
      return {
        label: s.name,
        data: values,
        borderColor: color,
        backgroundColor: color,
        borderWidth: 2,
        borderJoinStyle: "round",
        borderCapStyle: "round",
        tension: 0,
        spanGaps: true,
        pointRadius: function (ctx) { return ctx.dataIndex === last || few ? 4 : 0; },
        pointHoverRadius: 5,
        pointHitRadius: 10,
        pointBorderColor: surface, // 2px surface ring keeps dots legible on crossings
        pointBorderWidth: 2,
      };
    });
    var showLegend = datasets.length >= 2;
    var endLabelsOn = datasets.length >= 2 && datasets.length <= 4 && root.clientWidth >= 520;
    var labelRoom = endLabelsOn ? 110 : 8;

    chart = new window.Chart(canvas, {
      type: "line",
      data: { labels: dates, datasets: datasets },
      plugins: [crosshair, endLabels],
      options: {
        responsive: true,
        maintainAspectRatio: false,
        animation: false,
        parsing: { xAxisKey: "x", yAxisKey: "y" },
        interaction: { mode: "index", intersect: false },
        layout: { padding: { right: labelRoom } },
        scales: {
          x: {
            ticks: { color: token("--chart-muted"), maxTicksLimit: root.clientWidth < 520 ? 4 : 8, maxRotation: 0 },
            grid: { display: false },
            border: { color: token("--chart-axis") },
          },
          y: {
            ticks: { color: token("--chart-muted"), callback: compactVnd },
            grid: { color: token("--chart-grid"), lineWidth: 1 },
            border: { display: false },
          },
        },
        plugins: {
          legend: {
            display: showLegend,
            position: "top",
            align: "start",
            labels: {
              color: token("--chart-text"),
              usePointStyle: true,
              boxWidth: 18,
              generateLabels: function (c) {
                return c.data.datasets.map(function (ds, i) {
                  return {
                    text: ds.label,
                    datasetIndex: i,
                    hidden: !c.isDatasetVisible(i),
                    pointStyle: "line",
                    strokeStyle: ds.borderColor,
                    fillStyle: ds.borderColor,
                    lineWidth: 2,
                    fontColor: token("--chart-text"),
                  };
                });
              },
            },
          },
          tooltip: {
            backgroundColor: surface,
            titleColor: token("--chart-text"),
            bodyColor: token("--chart-text"),
            footerColor: token("--chart-text-secondary"),
            borderColor: token("--chart-axis"),
            borderWidth: 1,
            usePointStyle: true,
            callbacks: {
              labelPointStyle: function () { return { pointStyle: "line", rotation: 0 }; },
              labelColor: function (item) {
                var color = item.dataset.borderColor;
                return { borderColor: color, backgroundColor: color, borderWidth: 2 };
              },
              label: function (item) {
                var raw = item.raw;
                if (raw.y === null) return null;
                var main = vnd(raw.y) + "  " + item.dataset.label;
                var extra = [];
                if (raw.original && raw.original > raw.y) extra.push("was " + vnd(raw.original));
                if (raw.member) extra.push("member " + vnd(raw.member));
                if (!extra.length) return main;
                // Narrow charts (phones): details on a second line so the box fits.
                return item.chart.width < 520
                  ? [main, "    " + extra.join(" · ")]
                  : main + "  (" + extra.join(" · ") + ")";
              },
            },
          },
        },
      },
    });
  }

  function load(days) {
    root.querySelectorAll("[data-days]").forEach(function (b) {
      b.setAttribute("aria-pressed", String(Number(b.getAttribute("data-days")) === days));
    });
    fetch(src + "?days=" + days, { credentials: "same-origin" })
      .then(function (r) { return r.json(); })
      .then(render);
  }

  root.querySelectorAll("[data-days]").forEach(function (button) {
    button.addEventListener("click", function () { load(Number(button.getAttribute("data-days"))); });
  });
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", function () {
    if (lastData) render(lastData);
  });
  load(Number(root.getAttribute("data-days") || 90));
})();
