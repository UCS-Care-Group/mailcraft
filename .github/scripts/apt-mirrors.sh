#!/usr/bin/env bash
# Gives the runner's apt more than one Ubuntu mirror, so one that is down (or whose name doesn't
# resolve) is skipped instead of failing the job. GitHub's Ubuntu images read
# /etc/apt/apt-mirrors.txt through apt's `mirror+file:` method, which tries the next line when one
# fails; the image lists only Azure's. Then `apt-get update` with retries. Run before any install.
set -euo pipefail
list=/etc/apt/apt-mirrors.txt
if [[ -f $list ]]; then
  case $(dpkg --print-architecture) in
    amd64 | i386) extra=(http://archive.ubuntu.com/ubuntu/ http://mirrors.edge.kernel.org/ubuntu/) ;;
    *) extra=(http://ports.ubuntu.com/ubuntu-ports/) ;;
  esac
  for m in "${extra[@]}"; do grep -qxF "$m" "$list" || echo "$m" | sudo tee -a "$list" >/dev/null; done
fi
# ponytail: three tries a few seconds apart; a longer outage still fails the job, as it should.
for try in 1 2 3; do
  sudo apt-get -o Acquire::Retries=3 --error-on=any update && exit 0
  sleep $((try * 10))
done
exit 1
