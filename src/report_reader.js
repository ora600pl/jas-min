// Progressive enhancement: the complete report is present without JavaScript.
(() => {
    const toc = document.querySelector('.toc');
    if (toc) {
        toc.classList.add('compact-toc');
        const toggle = document.createElement('button');
        toggle.type = 'button';
        toggle.className = 'toc-toggle';
        toggle.textContent = 'Show finding titles';
        toggle.setAttribute('aria-expanded', 'false');
        toggle.addEventListener('click', () => {
            const compact = toc.classList.toggle('compact-toc');
            toggle.setAttribute('aria-expanded', String(!compact));
            toggle.textContent = compact ? 'Show finding titles' : 'Sections only';
        });
        toc.querySelector('h2').after(toggle);
    }
    const controls = document.createElement('div');
    controls.className = 'reader-controls';
    controls.setAttribute('role', 'group');
    controls.setAttribute('aria-label', 'Report reading detail');
    for (const [label, expanded] of [['Expand all evidence', true], ['Collapse evidence', false]]) {
        const button = document.createElement('button');
        button.type = 'button';
        button.textContent = label;
        button.addEventListener('click', () => {
            document.querySelectorAll('details').forEach(detail => { detail.open = expanded; });
        });
        controls.append(button);
    }
    const firstSection = document.querySelector('h2.section-title');
    if (firstSection) firstSection.before(controls);

    // Links to evidence inside closed details must reveal the target, including
    // a fragment opened in a fresh tab or reached through browser back/forward.
    function revealFragment() {
        let id;
        try { id = decodeURIComponent(location.hash.slice(1)); } catch (_) { return; }
        if (!id) return;
        const target = document.getElementById(id);
        if (!target) return;
        let ancestor = target.parentElement;
        while (ancestor) {
            if (ancestor.tagName === 'DETAILS') ancestor.open = true;
            ancestor = ancestor.parentElement;
        }
        target.scrollIntoView({block: 'start'});
        if (toc) toc.querySelectorAll('a').forEach(link => {
            if (link.hash === location.hash) link.setAttribute('aria-current', 'location');
            else link.removeAttribute('aria-current');
        });
    }
    addEventListener('hashchange', revealFragment);
    document.addEventListener('click', event => {
        const link = event.target.closest('a[href^="#"]');
        if (link && link.hash === location.hash) revealFragment();
    });
    revealFragment();

    let printState = [];
    addEventListener('beforeprint', () => {
        printState = [...document.querySelectorAll('details')].map(detail => [detail, detail.open]);
        printState.forEach(([detail]) => { detail.open = true; });
    });
    addEventListener('afterprint', () => {
        printState.forEach(([detail, open]) => { detail.open = open; });
    });
})();
