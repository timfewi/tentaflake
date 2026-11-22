import { Refusal, identity } from './ledger.mjs';

const providers = new Map([
  ['openai-responses', 'openai'],
  ['anthropic-messages', 'anthropic'],
  ['opencode-go-chat-completions', 'opencode-go'],
]);
function exactGrant(value) {
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).some(
      (k) =>
        ![
          'version',
          'revision',
          'provider',
          'model',
          'protocol',
          'endpoint',
          'pricing',
          'tools',
          'budget',
          'brokerBinding',
        ].includes(k),
    ) ||
    (value.brokerBinding !== undefined && typeof value.brokerBinding !== 'boolean') ||
    value.version !== 1 ||
    !identity(value.revision) ||
    !identity(value.provider) ||
    typeof value.model !== 'string' ||
    !value.model ||
    value.model.length > 200 ||
    providers.get(value.protocol) !== value.provider ||
    typeof value.endpoint !== 'string' ||
    !value.endpoint.startsWith('http') ||
    !value.pricing ||
    value.pricing.currency !== 'USD' ||
    !identity(value.pricing.revision) ||
    !Number.isSafeInteger(value.pricing.input_microusd_per_million) ||
    value.pricing.input_microusd_per_million <= 0 ||
    !Number.isSafeInteger(value.pricing.output_microusd_per_million) ||
    value.pricing.output_microusd_per_million <= 0 ||
    !Array.isArray(value.tools) ||
    new Set(value.tools).size !== value.tools.length ||
    value.tools.some((t) => !identity(t)) ||
    !value.budget ||
    !Number.isSafeInteger(value.budget.max_output_tokens) ||
    value.budget.max_output_tokens <= 0
  )
    throw new Refusal('Invalid versioned provider grant');
  const copy = structuredClone(value);
  Object.freeze(copy.pricing);
  Object.freeze(copy.tools);
  Object.freeze(copy.budget);
  return Object.freeze(copy);
}
export function isLegacySingleton(config, grant) {
  return config.grants.length === 1 && !grant.brokerBinding && grant.model === config.model
    && grant.provider === 'openai' && grant.protocol === 'openai-responses';
}

export function selectGrant(config, binding) {
  if (binding === undefined) {
    if (config.grants.length !== 1) throw new Refusal('Explicit provider binding required', 400);
    return config.grants[0];
  }
  if (
    !binding ||
    typeof binding !== 'object' ||
    Array.isArray(binding) ||
    Object.keys(binding).some((k) => !['revision', 'provider', 'model', 'protocol'].includes(k)) ||
    !['revision', 'provider', 'model', 'protocol'].every((k) => typeof binding[k] === 'string')
  )
    throw new Refusal('Invalid provider binding', 400);
  const grant = config.grants.find((candidate) =>
    ['revision', 'provider', 'model', 'protocol'].every((k) => candidate[k] === binding[k]),
  );
  if (!grant) throw new Refusal('Provider binding is not granted', 403);
  return grant;
}

export function configuredGrants(config, tools) {
  const grants = config.grants ?? [
    {
      version: 1,
      revision: 'legacy-single-model',
      provider: 'openai',
      model: config.model,
      protocol: 'openai-responses',
      endpoint: config.brokerBaseUrl + '/responses',
      pricing: {
        revision: 'legacy-host-policy',
        currency: 'USD',
        input_microusd_per_million: 1,
        output_microusd_per_million: 1,
      },
      tools,
      budget: { max_output_tokens: 4096 },
    },
  ];
  if (!Array.isArray(grants) || !grants.length) throw new Refusal('At least one provider grant is required');
  const checked = grants.map(exactGrant);
  if (new Set(checked.map((grant) => grant.revision)).size !== checked.length)
    throw new Refusal('Provider grant revisions must be unique');
  return Object.freeze(checked);
}
