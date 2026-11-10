import { execFileSync } from 'node:child_process';

const output = execFileSync('pnpm', ['why', 'effect'], { encoding: 'utf8' });
const match = /Found (\d+) versions? of effect/.exec(output);
if (match?.[1] !== '1') {
  process.stderr.write(
    `Expected exactly one version of effect.\n${output}\n` +
      'A second copy makes every Code Mode tool call fail.\n',
  );
  process.exit(1);
}
process.stdout.write('effect: one version\n');
