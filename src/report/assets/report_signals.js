// Native SVG, links and tables remain usable without JS or network access.
(() => {
  document.querySelectorAll('.signal-atlas').forEach(atlas => {
    const panels = [...atlas.querySelectorAll('[data-signal-panel]')];
    const project = atlas.querySelector('[data-signal-project]');
    const fit = atlas.querySelector('[data-signal-fit]');
    if (!panels.length || !project || !fit) return;
    atlas.classList.add('signal-enhanced');
    atlas.querySelector('.signal-controls').hidden = false;
    function show(id) {
      panels.forEach(panel => { panel.hidden = panel.dataset.signalPanel !== id; panel.open = true; });
      fit.value = id;
    }
    function chooseProject(preferred) {
      const previousLabel = panels.find(p => p.dataset.signalPanel === fit.value)?.dataset.label;
      const matching = panels.filter(p => p.dataset.project === project.value);
      fit.replaceChildren(...matching.map(panel => {
        const option = document.createElement('option');
        option.value = panel.dataset.signalPanel;
        option.textContent = panel.dataset.label;
        return option;
      }));
      show(preferred || matching.find(p => p.dataset.label === previousLabel)?.dataset.signalPanel || matching[0].dataset.signalPanel);
    }
    project.addEventListener('change', () => chooseProject());
    fit.addEventListener('change', () => show(fit.value));
    chooseProject(panels[0].dataset.signalPanel);
    function reveal() {
      let id;
      try { id = decodeURIComponent(location.hash.slice(1)); } catch (_) { return; }
      const target = document.getElementById(id);
      const panel = target?.closest('[data-signal-panel]');
      if (!panel || !atlas.contains(panel)) return;
      project.value = panel.dataset.project;
      chooseProject(panel.dataset.signalPanel);
      if (target.tagName === 'DETAILS') target.open = true;
      for (let parent = target.parentElement; parent; parent = parent.parentElement) {
        if (parent.tagName === 'DETAILS') parent.open = true;
      }
      target.scrollIntoView({block:'center'});
    }
    addEventListener('hashchange', reveal);
    atlas.addEventListener('click', event => {
      const link = event.target.closest('a[href^="#"]');
      if (link && link.getAttribute('href') === location.hash) reveal();
    });
    reveal();
  });
})();
