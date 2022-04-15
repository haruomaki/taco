function _adjust() {
    // const s = getComputedStyle(document.body);
    // const [mt, mr, mb, ml] = [s.marginTop, s.marginRight, s.marginBottom, s.marginLeft].map(parseFloat);
    // const [bt, br, bb, bl] = [s.borderTop, s.borderRight, s.borderBottom, s.borderLeft].map(parseFloat);
    // const width = ml + bl + document.body.scrollWidth + br + mr;
    // const height = mt + bt + document.body.scrollHeight + bb + mb;

    // const width = document.documentElement.scrollWidth;
    // const height = document.documentElement.scrollHeight;

    const width = document.documentElement.offsetWidth;
    const height = document.documentElement.offsetHeight;

    _rpc_adjustWindowToContent(
        width * 2,
        height * 2
    );
}

// Detect Document Height Change (https://stackoverflow.com/a/14901150/17257177)
onload = function () {
    // create an Observer instance
    const resizeObserver = new ResizeObserver(entries => _adjust());

    // start observing a DOM node
    resizeObserver.observe(document.body)
}
