#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
const root = process.env.DELIVERY_TEST_ROOT;
if (!root) throw new Error('isolated delivery fixture requires DELIVERY_TEST_ROOT');
const args = process.argv.slice(2);
fs.appendFileSync(path.join(root, 'gh-log'), `${JSON.stringify(args)}\n`);
if (args[0] === 'repo' && args[1] === 'view') {
  console.log(JSON.stringify({url:'https://github.com/fixture/delivery',defaultBranchRef:{name:'main'}}));
} else if (args[0] === 'pr' && args[1] === 'list') {
  console.log(fs.existsSync(path.join(root, 'gh-created')) ? JSON.stringify([{url:'https://github.com/fixture/delivery/pull/1',state:'OPEN',baseRefName:'main',headRefOid:fs.readFileSync(path.join(root,'gh-head'),'utf8')}]) : '[]');
} else if (args[0] === 'pr' && args[1] === 'create') {
  if (fs.existsSync(path.join(root, 'gh-hang'))) {
    fs.writeFileSync(path.join(root, 'gh-running'), String(process.pid));
    await new Promise(() => { setInterval(() => {}, 1000); });
  }
  fs.writeFileSync(path.join(root, 'gh-created'), JSON.stringify(args));
  console.log('https://github.com/fixture/delivery'); // wrapper/help noise is not the PR identity
  console.log('https://github.com/fixture/delivery/pull/1');
} else { throw new Error(`unapproved gh invocation: ${JSON.stringify(args)}`); }
