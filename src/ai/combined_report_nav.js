(() => {
    const nav = document.querySelector('.edition-nav');
    const links = [...nav.querySelectorAll('a')];
    const sections = [...document.querySelectorAll('[data-menu-section]')];
    let scheduled = false;

    function highlight() {
        const navBottom = nav.getBoundingClientRect().bottom + 24;
        let current = sections[0];
        for (const section of sections) {
            if (section.getBoundingClientRect().top > navBottom) break;
            current = section;
        }
        if (window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 2) {
            current = sections.at(-1);
        }
        for (const link of links) {
            if (link.hash === '#' + current.dataset.menuSection) {
                link.setAttribute('aria-current', 'location');
            } else {
                link.removeAttribute('aria-current');
            }
        }
        scheduled = false;
    }

    function schedule() {
        if (scheduled) return;
        scheduled = true;
        requestAnimationFrame(highlight);
    }

    window.addEventListener('scroll', schedule, { passive: true });
    window.addEventListener('resize', schedule);
    window.addEventListener('pageshow', schedule);
    window.addEventListener('hashchange', schedule);
    highlight();
})();
