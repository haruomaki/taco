function _adjust() {
    // NOTE: `width` should be calculated as follows so that it works well
    // even if the page width is specified in stylesheets or the DPI is subject to change.

    const s = getComputedStyle(document.body);
    const [mt, mr, mb, ml] = [s.marginTop, s.marginRight, s.marginBottom, s.marginLeft].map(parseFloat);
    const [bt, br, bb, bl] = [s.borderTop, s.borderRight, s.borderBottom, s.borderLeft].map(parseFloat);
    const width = ml + bl + document.body.scrollWidth + br + mr;
    // const height = mt + bt + document.body.scrollHeight + bb + mb;

    // const width = document.documentElement.scrollWidth;
    // const height = document.documentElement.scrollHeight;

    // const width = document.documentElement.offsetWidth;
    // const height = document.documentElement.offsetHeight;

    // const width = ml + document.body.getBoundingClientRect().width + mr;
    const height = document.documentElement.getBoundingClientRect().height;

    _rpc_adjustWindowToContent(width, height);
}

// Detect Document Height Change (https://stackoverflow.com/a/14901150/17257177)
onload = function () {
    // create an Observer instance
    const resizeObserver = new ResizeObserver(entries => _adjust());

    // start observing a DOM node
    resizeObserver.observe(document.documentElement)
}
