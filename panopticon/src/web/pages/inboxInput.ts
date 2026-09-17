export interface OwnerRepo {
  owner: string;
  repo: string;
}

export interface ParsedPrInput {
  owner: string;
  repo: string;
  number: number;
}

const GITHUB_URL_PATTERN =
  /^https?:\/\/github\.com\/([^/\s]+)\/([^/\s]+)\/pull\/(\d+)(?:[/?#].*)?$/i;
const OWNER_REPO_NUMBER_PATTERN = /^([^/\s#]+)\/([^/\s#]+)#(\d+)$/;
const BARE_NUMBER_PATTERN = /^#?(\d+)$/;

// Accepts a bare number ("1234"), "owner/repo#1234", or a GitHub PR URL. A
// bare number needs a default owner/repo, usually the first inbox entry;
// without one, only the fully qualified forms parse.
export function parsePrInput(
  raw: string,
  defaultOwnerRepo: OwnerRepo | null,
): ParsedPrInput | null {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return null;

  const urlMatch = GITHUB_URL_PATTERN.exec(trimmed);
  if (urlMatch) {
    const [, owner, repo, numberText] = urlMatch;
    if (owner !== undefined && repo !== undefined && numberText !== undefined) {
      return { owner, repo, number: Number(numberText) };
    }
  }

  const shortMatch = OWNER_REPO_NUMBER_PATTERN.exec(trimmed);
  if (shortMatch) {
    const [, owner, repo, numberText] = shortMatch;
    if (owner !== undefined && repo !== undefined && numberText !== undefined) {
      return { owner, repo, number: Number(numberText) };
    }
  }

  const bareMatch = BARE_NUMBER_PATTERN.exec(trimmed);
  if (bareMatch) {
    const numberText = bareMatch[1];
    if (numberText !== undefined && defaultOwnerRepo !== null) {
      return {
        owner: defaultOwnerRepo.owner,
        repo: defaultOwnerRepo.repo,
        number: Number(numberText),
      };
    }
    return null;
  }

  return null;
}
