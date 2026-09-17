import type { LanguageRegistration } from 'shiki/core';

import grammar from './sola.tmLanguage.json';

const sola: LanguageRegistration = {
  ...grammar,
  name: 'sola',
  aliases: ['dsl'],
};

export default [sola];
