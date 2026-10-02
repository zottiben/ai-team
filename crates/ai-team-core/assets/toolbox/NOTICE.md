# Bundled ai-toolbox catalogue

Source: https://github.com/zottiben/ai-toolbox
Revision: `90659e82f0d040315ff99cfbd765d8798eb3e085`
Engine package: `ai-toolbox-core` 0.1.4 (pinned separately in Cargo.toml).

The upstream workspace and engine declare the MIT licence. This revision has no
root LICENSE file. The following standard MIT terms accompany this redistribution;
upstream attribution is retained in the assets. Copyright belongs to the respective
ai-toolbox contributors. The separately vendored TypeSafe skill retains its own
`skills/typesafe-ai/LICENSE` and `UPSTREAM.md` verbatim.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Reproduction

`python3 tools/vendor-toolbox.py /path/to/ai-toolbox` extracts tracked blobs from
the pinned revision, not the working tree. `FILES` records SHA-256 and executable
modes. The build embeds these bytes and notices; runtime extraction uses a private
temporary directory and never discovers a sibling checkout, registry, home config,
or standalone toolbox installation. To update, review the engine pin, script pin,
asset diff, notices and capability coverage together.
