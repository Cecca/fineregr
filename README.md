# `fineregr` monitoring for regressions throughout Git history

`fineregr` executes arbirary commands against the shapshots of each and every commit in
a git repositoy, using [hyperfine](https://github.com/sharkdp/hyperfine) to
measure timings accurately, and [Vega lite](https://vega.github.io/vega-lite/)
to visualize the results.