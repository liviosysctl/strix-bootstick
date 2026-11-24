/* The bootstick app: one window, one way through it.

     loading -> pick -> (confirm) -> work -> done
                  ^                    |
                  +---- failed <-------+

   The Rust side does the work (commands release, sticks, start, cancel,
   restart, quit; events step, finished, close-requested). This file shows it
   with the components of StrixUI. Every name that comes from outside (the
   release, the sticks, error texts) goes through esc() before it is HTML. */
(function () {
  'use strict';

  var tauri = window.__TAURI__;
  var invoke = tauri.core.invoke;
  var listen = tauri.event.listen;
  var appWindow = tauri.window.getCurrentWindow();
  var Kit = window.Kit;
  var S = window.Strings.pick((navigator.languages && navigator.languages[0]) || navigator.language);

  var STAGES = ['stick', 'download', 'write', 'verify'];
  /* How often the stick list is read while it is on screen */
  var POLL = 2000;
  /* Sizes sticks are sold in, in GB: "at least 4 GB" reads better than "at least 2.97 GB" */
  var STICK_SIZES = [1, 2, 4, 8, 16, 32, 64, 128, 256];
  /* Rate and time left are measured over this window, and shown once it holds this much */
  var RATE_WINDOW = 8000;
  var RATE_MIN = 2000;

  var panel = document.getElementById('panel');
  var scrim = document.getElementById('scrim');
  var steps = document.getElementById('steps');
  var plate = document.getElementById('plate');

  var state = {
    view: '',
    release: null,
    /* The stick list is on screen and being refreshed */
    picking: false,
    pollTimer: 0,
    sticks: [],
    sticksKey: null,
    chosen: null,
    note: '',
    /* The run */
    stickLabel: '',
    stage: 'download',
    samples: [],
    stopping: false,
    closing: false,
    /* The open dialog's answer function */
    answer: null
  };

  /* ---- small helpers ---- */

  function esc(text) {
    return String(text).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  function fill(text, values) {
    return text.replace(/\{(\w+)\}/g, function (all, key) {
      return values && key in values ? values[key] : all;
    });
  }

  function number(value, digits) {
    return value.toLocaleString(S.locale, { minimumFractionDigits: digits, maximumFractionDigits: digits });
  }

  /* Sizes the way stick packages print them: 1 GB = 1000^3 bytes */
  function size(bytes) {
    var gb = bytes / 1e9;
    return number(gb, gb >= 10 ? 0 : 1) + ' GB';
  }

  function rate(bytesPerSecond) {
    var mb = bytesPerSecond / 1e6;
    return fill(S.rate, { rate: number(mb, mb >= 10 ? 0 : 1) + ' MB' });
  }

  /* The smallest usual stick that holds the release */
  function stickSize(bytes) {
    for (var i = 0; i < STICK_SIZES.length; i++) {
      if (STICK_SIZES[i] * 1e9 >= bytes) return STICK_SIZES[i] + ' GB';
    }
    return size(bytes);
  }

  function stickLabel(stick) {
    var letters = stick.letters.length ? ' (' + stick.letters.join(', ') + ')' : '';
    return (stick.name || S.unnamed) + ', ' + size(stick.size) + letters;
  }

  function button(action, label, kind, more) {
    return '<button class="k-btn' + (kind ? ' k-btn-' + kind : '') + '" data-do="' + action + '"' + (more || '') + '><span>' + esc(label) + '</span></button>';
  }

  /* ---- the frame around every view ---- */

  function drawSteps() {
    steps.innerHTML = STAGES.map(function (stage) {
      var name = S['step' + stage.charAt(0).toUpperCase() + stage.slice(1)];
      return '<li><span>' + esc(name) + '</span><i class="k-icon" data-icon="check"></i></li>';
    }).join('');
    Kit.hydrate(steps);
  }

  /* how: undefined while the stage is in work, 'over' when all are done, 'failed' when it broke */
  function markSteps(stage, how) {
    var at = STAGES.indexOf(stage);
    steps.style.setProperty('--at', at);
    steps.toggleAttribute('data-over', how === 'over');
    steps.toggleAttribute('data-failed', how === 'failed');
    Array.prototype.forEach.call(steps.children, function (item, n) {
      item.setAttribute('data-state', how === 'over' || n < at ? 'done' : n === at ? 'active' : '');
    });
  }

  /* The plate steps back while the app works and is lit when the stick is ready */
  function light(how) {
    plate.toggleAttribute('data-dim', how === 'dim');
    plate.toggleAttribute('data-lit', how === 'lit');
  }

  function show(view, parts) {
    state.view = view;
    panel.innerHTML =
      '<header class="app-head"><h2>' + esc(parts.title) + '</h2><p>' + esc(parts.lead || '') + '</p></header>' +
      '<div class="app-body"' + (parts.top ? ' data-top' : '') + '>' + (parts.body || '') + '</div>' +
      '<footer class="app-foot"><p class="app-hint">' + esc(parts.hint || '') + '</p><div class="app-actions">' + (parts.actions || '') + '</div></footer>';
    Kit.hydrate(panel);
    var first = panel.querySelector('[data-focus]');
    if (first) first.focus({ preventScroll: true });
  }

  /* New title and lead inside the same view, as the stages of the run pass */
  function setHead(title, lead) {
    var head = panel.querySelector('.app-head');
    head.setAttribute('data-swap', '');
    head.innerHTML = '<h2>' + esc(title) + '</h2><p>' + esc(lead) + '</p>';
  }

  /* A question over the whole window. Resolves true for yes, false for no,
     Escape, or when the app closes it because the question is moot. */
  function ask(question) {
    closeDialog();
    scrim.innerHTML =
      '<div class="k-dialog k-surface" role="dialog" aria-modal="true" aria-labelledby="dialog-title">' +
      '<h3 id="dialog-title">' + esc(question.title) + '</h3><p>' + esc(question.text) + '</p>' +
      '<footer>' + button('no', question.no, 'quiet') + button('yes', question.yes, 'danger') + '</footer></div>';
    scrim.hidden = false;
    scrim.querySelector('[data-do="no"]').focus({ preventScroll: true });
    return new Promise(function (resolve) { state.answer = resolve; });
  }

  function closeDialog(answer) {
    if (!state.answer) return;
    var resolve = state.answer;
    state.answer = null;
    scrim.hidden = true;
    scrim.innerHTML = '';
    resolve(answer === true);
  }

  /* ---- loading: which release is there ---- */

  function load() {
    markSteps('stick');
    light();
    show('loading', {
      title: S.loadingTitle,
      lead: S.loadingLead,
      body: '<div class="app-wait"><div class="k-progress" data-indeterminate role="progressbar"><i></i></div></div>'
    });
    invoke('release').then(function (release) {
      state.release = release;
      document.getElementById('name').textContent = release.name;
      document.getElementById('word').textContent = release.name;
      document.getElementById('edition').textContent = fill(S.edition, { version: release.version, size: size(release.size) });
      document.querySelector('.app-word').setAttribute('data-in', '');
      appWindow.setTitle(fill(S.windowTitle, { name: release.name })).catch(function () {});
      pick('');
    }, failed);
  }

  /* ---- pick: the stick list, read again every two seconds ---- */

  function pick(note) {
    state.note = note;
    state.picking = true;
    state.sticksKey = null;
    markSteps('stick');
    light();
    poll();
  }

  function poll() {
    clearTimeout(state.pollTimer);
    invoke('sticks').then(function (sticks) {
      if (!state.picking) return;
      /* An open question names a stick: the list waits until it is answered */
      if (!state.answer) drawSticks(sticks);
      state.pollTimer = setTimeout(poll, POLL);
    }, function (failure) {
      if (state.picking) failed(failure);
    });
  }

  function leavePick() {
    state.picking = false;
    clearTimeout(state.pollTimer);
  }

  function chosenStick() {
    return state.sticks.filter(function (stick) { return stick.number === state.chosen && !stick.problem; })[0];
  }

  function drawSticks(sticks) {
    var key = JSON.stringify(sticks);
    if (state.view === 'pick' && key === state.sticksKey) return;
    state.sticksKey = key;
    state.sticks = sticks;
    var usable = sticks.filter(function (stick) { return !stick.problem; });
    /* One usable stick is the one that is meant; with several the user says which */
    if (!chosenStick()) state.chosen = usable.length === 1 ? usable[0].number : null;

    var body;
    if (sticks.length) {
      body = '<ul class="k-list app-sticks" role="listbox">' + sticks.map(function (stick) {
        var aside = stick.problem === 'too-small' ? S.tooSmall : stick.problem === 'too-large' ? S.tooLarge : stick.letters.join(', ');
        return '<li class="k-row k-enter" role="option" data-stick="' + stick.number + '"' +
          ' aria-selected="' + (stick.number === state.chosen) + '"' + (stick.problem ? ' aria-disabled="true"' : '') + '>' +
          '<i class="k-icon" data-icon="usb"></i><span class="k-grow">' + esc(stick.name || S.unnamed) + '</span>' +
          '<small>' + esc(aside) + '</small><span>' + esc(size(stick.size)) + '</span></li>';
      }).join('') + '</ul>';
    } else {
      body = '<div class="app-wait"><i class="k-icon" data-icon="usb"></i><div class="k-empty">' + esc(S.waitNote) + '</div>' +
        '<div class="k-progress" data-indeterminate role="progressbar"><i></i></div></div>';
    }
    show('pick', {
      title: usable.length ? S.pickTitle : S.waitTitle,
      lead: usable.length
        ? fill(S.pickLead, { name: state.release.name })
        : fill(S.waitLead, { size: stickSize(state.release.size) }),
      top: sticks.length > 0,
      body: body,
      hint: state.note,
      actions: button('go', S.go, 'primary', state.chosen == null ? ' disabled' : ' data-focus')
    });
    Array.prototype.forEach.call(panel.querySelectorAll('.k-row'), function (row, n) {
      row.style.setProperty('--i', n);
    });
  }

  function choose(number) {
    state.chosen = number;
    Array.prototype.forEach.call(panel.querySelectorAll('.k-row'), function (row) {
      row.setAttribute('aria-selected', +row.getAttribute('data-stick') === number);
    });
    panel.querySelector('[data-do="go"]').disabled = false;
  }

  /* Arrow keys in the list: the next stick that can be chosen */
  function chooseNext(direction) {
    var usable = state.sticks.filter(function (stick) { return !stick.problem; });
    if (!usable.length) return;
    var at = usable.indexOf(chosenStick());
    var next = at < 0 ? (direction > 0 ? 0 : usable.length - 1) : (at + direction + usable.length) % usable.length;
    choose(usable[next].number);
  }

  function go() {
    var stick = chosenStick();
    if (!stick || state.answer) return;
    ask({
      title: S.confirmTitle,
      text: fill(S.confirmText, { stick: stickLabel(stick) }),
      yes: S.confirmYes,
      no: S.cancel
    }).then(function (yes) {
      if (yes && state.picking) work(stick);
    });
  }

  /* ---- work: download, write, check ---- */

  function work(stick) {
    leavePick();
    state.stickLabel = stickLabel(stick);
    state.stage = 'download';
    state.samples = [];
    state.stopping = false;
    state.closing = false;
    markSteps('download');
    light('dim');
    show('work', {
      title: fill(S.downloadTitle, { name: state.release.name }),
      lead: S.downloadLead,
      body:
        '<div class="app-meter">' +
        '<div class="app-percent"><span class="k-roll" id="percent"></span><i>%</i></div>' +
        '<div class="k-progress" id="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" data-indeterminate><i></i></div>' +
        '<div class="app-figures"><span id="amount">' + esc(S.prepare) + '</span><span id="left"></span></div>' +
        '</div>',
      actions: button('stop', S.cancel, 'quiet')
    });
    Kit.roll(document.getElementById('percent'), '0');
    invoke('start', { disk: stick.number }).catch(failed);
  }

  function onStep(step) {
    if (state.view !== 'work') return;
    var bar = document.getElementById('bar');
    if (step.stage !== state.stage) {
      state.stage = step.stage;
      state.samples = [];
      markSteps(step.stage);
      setHead(step.stage === 'write' ? S.writeTitle : S.verifyTitle, step.stage === 'write' ? S.writeLead : S.verifyLead);
      /* A fresh bar, so it starts at zero instead of running back from 100 */
      bar.style.setProperty('--p', '0%');
      bar.innerHTML = '<i></i>';
    }
    var share = step.total ? step.done / step.total : 1;
    bar.removeAttribute('data-indeterminate');
    bar.style.setProperty('--p', share * 100 + '%');
    bar.setAttribute('aria-valuenow', Math.floor(share * 100));
    Kit.roll(document.getElementById('percent'), String(Math.floor(share * 100)));
    drawFigures(step);
  }

  function drawFigures(step) {
    var now = performance.now();
    state.samples.push({ at: now, done: step.done });
    while (state.samples.length > 2 && now - state.samples[0].at > RATE_WINDOW) state.samples.shift();
    var first = state.samples[0];
    var span = now - first.at;
    var speed = span >= RATE_MIN ? (step.done - first.done) / (span / 1000) : 0;

    var amount = fill(S.amount, { done: size(step.done), total: size(step.total) });
    document.getElementById('amount').textContent = speed > 0 ? amount + ', ' + rate(speed) : amount;
    if (state.stopping) return;
    var left = '';
    if (speed > 0 && step.done < step.total) {
      var seconds = (step.total - step.done) / speed;
      left = seconds < 45 ? S.leftLess : seconds < 90 ? S.leftOne : fill(S.leftMany, { n: Math.round(seconds / 60) });
    }
    document.getElementById('left').textContent = left;
  }

  /* Stop the run, from its Cancel button or because the window should close */
  function stop(closing) {
    if (state.stopping) {
      state.closing = state.closing || closing;
      return;
    }
    if (state.answer) return;
    ask({
      title: S.stopTitle,
      text: state.stage === 'download' ? S.stopDownload : S.stopWrite,
      yes: S.stopYes,
      no: S.stopNo
    }).then(function (yes) {
      /* The run may have ended while the question was open */
      if (!yes || state.view !== 'work') return;
      state.stopping = true;
      state.closing = closing;
      panel.querySelector('[data-do="stop"]').disabled = true;
      document.getElementById('left').textContent = S.stopping;
      invoke('cancel');
    });
  }

  function onFinished(result) {
    closeDialog();
    if (result.outcome === 'done') {
      done(result);
    } else if (result.outcome === 'cancelled') {
      if (state.closing) invoke('quit');
      else pick(result.stage === 'download' ? '' : S.stopped);
    } else {
      failed(result);
    }
  }

  /* ---- done ---- */

  function done(result) {
    var release = state.release;
    markSteps('verify', 'over');
    light('lit');
    var text;
    if (result.bootNext) {
      text = '<p class="aside">' + esc(S.doneNext) + '</p><p>' + esc(S.doneNextMore + ' ' + S.doneOther) + '</p>';
    } else {
      var why = { 'not-uefi': S.whyNotUefi, 'sector-size': S.whySectorSize, firmware: S.whyFirmware }[result.noBootNext];
      text = '<p class="aside">' + esc(S.doneMenu) + '</p><p>' + esc(S.doneKeys) + '</p>' +
        '<div class="app-keys"><span class="k-kbd">F12</span><span class="k-kbd">F9</span><span class="k-kbd">F2</span><span class="k-kbd">Esc</span></div>' +
        (why ? '<p>' + esc(why) + '</p>' : '');
    }
    if (release.secureBootOff) text += '<p>' + esc(S.secureOff) + '</p>';
    show('done', {
      title: S.doneTitle,
      lead: fill(S.doneLead, { name: release.name, version: release.version, stick: state.stickLabel }),
      body: '<div class="app-result"><div class="app-seal"><i class="k-icon" data-icon="check"></i></div><div>' + text + '</div></div>',
      actions: button('again', S.again, 'quiet') +
        (result.bootNext
          ? button('quit', S.close) + button('restart', S.restart, 'primary', ' data-focus')
          : button('quit', S.close, 'primary', ' data-focus'))
    });
  }

  function restart() {
    var buttons = panel.querySelectorAll('.app-actions .k-btn');
    Array.prototype.forEach.call(buttons, function (b) { b.disabled = true; });
    var own = panel.querySelector('[data-do="restart"]');
    own.setAttribute('data-loading', '');
    own.firstChild.textContent = S.restarting;
    invoke('restart').catch(failed);
  }

  /* ---- failed ---- */

  function explain(failure) {
    var release = state.release;
    if (failure.kind === 'network') return [S.failNetworkTitle, S.failNetworkLead];
    if (failure.kind === 'outdated') return [S.failOutdatedTitle, S.failOutdatedLead];
    if (failure.kind === 'manifest' || failure.kind === 'image') return [S.failReleaseTitle, S.failReleaseLead];
    if (failure.kind === 'too-small') return [S.failSmallTitle, fill(S.failSmallLead, { size: stickSize(release.size) })];
    if (failure.kind === 'refused') return [S.failRefusedTitle, S.failStickLead];
    if (failure.kind === 'damaged') {
      return failure.stage === 'verify' ? [S.failFaultyTitle, S.failFaultyLead] : [S.failDamagedTitle, S.failDamagedLead];
    }
    if (failure.stage === 'download') return [S.failDownloadTitle, fill(S.failDownloadLead, { size: size(release.size) })];
    if (failure.stage === 'write' || failure.stage === 'verify') return [S.failStickTitle, S.failStickLead];
    return [S.failTitle, S.failLead];
  }

  function failed(failure) {
    /* Tauri itself answers with a plain text when a command cannot run at all */
    if (typeof failure !== 'object' || !failure) failure = { kind: '', stage: '', detail: String(failure) };
    leavePick();
    closeDialog();
    light();
    var stage = STAGES.indexOf(failure.stage) >= 0 ? failure.stage : state.view === 'work' ? state.stage : 'stick';
    markSteps(stage, 'failed');
    var words = explain(failure);
    show('failed', {
      title: words[0],
      lead: words[1],
      body: '<pre class="app-detail">' + esc(failure.detail || '') + '</pre>',
      actions: button('quit', S.close, 'quiet') + button('retry', S.retry, 'primary', ' data-focus')
    });
  }

  /* Without a release there is nothing to pick a stick for */
  function retry() {
    if (state.release) pick('');
    else load();
  }

  /* ---- input ---- */

  function close() {
    if (state.view === 'work') stop(true);
    else invoke('quit');
  }

  var actions = {
    go: go,
    stop: function () { stop(false); },
    again: function () { pick(''); },
    retry: retry,
    restart: restart,
    quit: function () { invoke('quit'); },
    yes: function () { closeDialog(true); },
    no: function () { closeDialog(false); }
  };

  document.addEventListener('click', function (event) {
    var doer = event.target.closest('[data-do]');
    if (doer && !doer.disabled) {
      actions[doer.getAttribute('data-do')]();
      return;
    }
    var row = event.target.closest('.k-row[data-stick]');
    if (row && row.getAttribute('aria-disabled') !== 'true') choose(+row.getAttribute('data-stick'));
  });

  document.addEventListener('dblclick', function (event) {
    var row = event.target.closest('.k-row[data-stick]');
    if (row && row.getAttribute('aria-disabled') !== 'true') go();
  });

  document.addEventListener('keydown', function (event) {
    /* The page is an app, not a document: no reload, print, find or zoom */
    var key = event.key.toLowerCase();
    if (event.key === 'F5' || event.key === 'F3' || event.key === 'F7' || (event.ctrlKey && 'rpfgjus+-0'.indexOf(key) >= 0)) {
      event.preventDefault();
      return;
    }
    if (state.answer) {
      if (event.key === 'Escape') closeDialog(false);
      return;
    }
    if (state.view !== 'pick') return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      chooseNext(event.key === 'ArrowDown' ? 1 : -1);
    } else if (event.key === 'Enter' && !event.target.closest('button')) {
      go();
    }
  });

  document.addEventListener('contextmenu', function (event) {
    if (!event.target.closest('.app-detail')) event.preventDefault();
  });

  document.getElementById('minimize').addEventListener('click', function () { appWindow.minimize(); });
  document.getElementById('close').addEventListener('click', close);

  listen('step', function (event) { onStep(event.payload); });
  listen('finished', function (event) { onFinished(event.payload); });
  listen('close-requested', function () { stop(true); });

  /* ---- start ---- */

  document.documentElement.lang = S.locale;
  document.title = S.subtitle;
  document.getElementById('subtitle').textContent = S.subtitle;
  document.getElementById('minimize').setAttribute('aria-label', S.minimize);
  document.getElementById('close').setAttribute('aria-label', S.close);
  drawSteps();
  load();
})();
