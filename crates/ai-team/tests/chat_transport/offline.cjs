// A fixture tripwire, not an OS sandbox. No provider endpoint should be contacted.
const fs = require('node:fs');
const net = require('node:net');
const deny = () => {
  fs.appendFileSync(process.env.BUILD_TEST_ROOT + '/network-denied', `${process.argv[1]}\n${new Error('unexpected network attempt').stack}\n`);
  throw new Error('offline transport fixture forbids network access');
};
net.Socket.prototype.connect = deny;
globalThis.fetch = deny;
