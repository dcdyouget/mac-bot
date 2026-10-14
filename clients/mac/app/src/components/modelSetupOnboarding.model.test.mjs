import assert from 'node:assert/strict'
import test from 'node:test'
import { modelSetupOnboardingSection } from './modelSetupOnboarding.model.ts'

test('routes first-run users to model setup when the main default is missing', () => {
  assert.equal(modelSetupOnboardingSection({ models: {} }, [], []), 'models')
  assert.equal(modelSetupOnboardingSection({ models: { main: null } }, [], [{ ref: 'provider/model' }]), 'models')
  assert.equal(modelSetupOnboardingSection({ models: { main: ' inherit ' } }, [], [{ ref: 'provider/model' }]), 'models')
})

test('routes to model setup when the configured main default is unavailable or disabled', () => {
  const settings = { models: { main: 'provider/model' } }

  assert.equal(modelSetupOnboardingSection(settings, [], []), 'models')
  assert.equal(modelSetupOnboardingSection(settings, [], [{ ref: 'provider/model', enabled: false }]), 'models')
})

test('keeps users with an available main default in their current page', () => {
  assert.equal(
    modelSetupOnboardingSection({ models: { main: 'provider/model' } }, [], [{ ref: 'provider/model' }]),
    null,
  )
})

test('respects a usable explicit main Bot model over an empty global default', () => {
  assert.equal(
    modelSetupOnboardingSection({ models: { main: null } }, [{ is_main: true, model: 'provider/override' }], [{ ref: 'provider/override' }]),
    null,
  )
})
