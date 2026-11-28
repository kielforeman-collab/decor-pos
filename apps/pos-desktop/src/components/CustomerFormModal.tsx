// New / Edit customer modal used by the Customers (Accounts) screen.
// Credit controls (charge account, credit limit, terms, finance rate,
// credit hold, portal access) are editable only for Manager / Admin
// (roles holding ArPayment); cashiers can still create and edit
// contact, address, pricing, tax-exempt, and counter rules.

import { useState, type FormEvent } from 'react'
import { createCustomer, updateCustomer } from '../api'
import type { CustomerInputPayload, CustomerSearchResult, PriceLevel } from '../types'

const CUSTOMER_TYPES = [
  { value: 'retail', label: 'Retail' },
  { value: 'contractor', label: 'Contractor' },
  { value: 'commercial', label: 'Commercial' },
]

const COMMON_TERMS = ['NET30', 'NET15', 'NET60', 'DUE_ON_RECEIPT', 'COD']

function canManageCredit(role: string): boolean {
  const r = role.toLowerCase()
  return r === 'manager' || r === 'admin'
}

export default function CustomerFormModal({
  customer,
  levels,
  userRole,
  onSaved,
  onClose,
}: {
  customer: CustomerSearchResult | null
  levels: PriceLevel[]
  userRole: string
  onSaved: (saved: CustomerSearchResult, created: boolean) => void
  onClose: () => void
}) {
  const isEdit = customer !== null
  const creditEditable = canManageCredit(userRole)

  const [accountNumber, setAccountNumber] = useState(customer?.account_number ?? '')
  const [customerType, setCustomerType] = useState(customer?.customer_type || 'retail')
  const [companyName, setCompanyName] = useState(customer?.company_name ?? '')
  const [firstName, setFirstName] = useState(customer?.first_name ?? '')
  const [lastName, setLastName] = useState(customer?.last_name ?? '')
  const [email, setEmail] = useState(customer?.email ?? '')
  const [phone, setPhone] = useState(customer?.phone ?? '')

  const [addressLine1, setAddressLine1] = useState(customer?.address_line1 ?? '')
  const [addressLine2, setAddressLine2] = useState(customer?.address_line2 ?? '')
  const [city, setCity] = useState(customer?.city ?? '')
  const [province, setProvince] = useState(customer?.province ?? '')
  const [postalCode, setPostalCode] = useState(customer?.postal_code ?? '')

  const [priceLevelId, setPriceLevelId] = useState(customer?.price_level_id ?? '')
  const [taxExemptCert, setTaxExemptCert] = useState(customer?.tax_exempt_cert ?? '')
  const [taxExemptExpires, setTaxExemptExpires] = useState(customer?.tax_exempt_expires ?? '')
  const [requirePo, setRequirePo] = useState(customer?.require_po_number ?? false)
  const [requireJob, setRequireJob] = useState(customer?.require_job_name ?? false)
  const [notes, setNotes] = useState(customer?.notes ?? '')

  const [chargeEnabled, setChargeEnabled] = useState(customer?.charge_enabled ?? false)
  const [creditLimit, setCreditLimit] = useState(
    customer?.credit_limit ? String(customer.credit_limit.amount) : '0.00',
  )
  const [paymentTerms, setPaymentTerms] = useState(customer?.payment_terms ?? 'NET30')
  const [termsDays, setTermsDays] = useState(
    customer?.terms_days != null ? String(customer.terms_days) : '',
  )
  const [financeRate, setFinanceRate] = useState(customer?.finance_rate_pct_monthly ?? '')
  const [creditHold, setCreditHold] = useState(customer?.credit_hold ?? false)
  const [portalEnabled, setPortalEnabled] = useState(customer?.portal_enabled ?? false)

  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleSubmit(e: FormEvent) {
    e.preventDefault()
    if (!companyName.trim() && !firstName.trim() && !lastName.trim()) {
      setError('Enter a company name or a first/last name.')
      return
    }
    const parsedDays = termsDays.trim() === '' ? null : Number(termsDays.trim())
    if (parsedDays !== null && (!Number.isInteger(parsedDays) || parsedDays < 0 || parsedDays > 365)) {
      setError('Terms days must be a whole number between 0 and 365.')
      return
    }

    const payload: CustomerInputPayload = {
      account_number: accountNumber.trim() || null,
      customer_type: customerType,
      company_name: companyName.trim() || null,
      first_name: firstName.trim() || null,
      last_name: lastName.trim() || null,
      email: email.trim() || null,
      phone: phone.trim() || null,
      address_line1: addressLine1.trim() || null,
      address_line2: addressLine2.trim() || null,
      city: city.trim() || null,
      province: province.trim() || null,
      postal_code: postalCode.trim() || null,
      notes: notes.trim() || null,
      price_level_id: priceLevelId || null,
      tax_exempt_cert: taxExemptCert.trim() || null,
      tax_exempt_expires: taxExemptExpires.trim() || null,
      require_po_number: requirePo,
      require_job_name: requireJob,
      // Keep stored credit controls untouched when not editable so the
      // server sees zero credit-control diff for Cashier sessions.
      charge_enabled: creditEditable ? chargeEnabled : (customer?.charge_enabled ?? false),
      credit_limit: creditEditable
        ? (creditLimit.trim() || '0.00')
        : (customer?.credit_limit ? String(customer.credit_limit.amount) : '0.00'),
      payment_terms: creditEditable
        ? (paymentTerms.trim() || 'NET30')
        : (customer?.payment_terms ?? 'NET30'),
      terms_days: creditEditable ? parsedDays : (customer?.terms_days ?? null),
      finance_rate_pct_monthly: creditEditable
        ? (financeRate.trim() || null)
        : (customer?.finance_rate_pct_monthly ?? null),
      credit_hold: creditEditable ? creditHold : (customer?.credit_hold ?? false),
      portal_enabled: creditEditable ? portalEnabled : (customer?.portal_enabled ?? false),
    }

    setBusy(true)
    setError(null)
    try {
      const saved = isEdit
        ? await updateCustomer(customer.id, payload)
        : await createCustomer(payload)
      onSaved(saved, !isEdit)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="modal-overlay" role="dialog" aria-modal="true" aria-label={isEdit ? 'Edit customer' : 'New customer'}>
      <div className="modal" style={{ maxWidth: 740, width: '94vw', maxHeight: '90vh', overflowY: 'auto' }}>
        <div className="modal-head">
          <h2>{isEdit ? `Edit customer · ${customer.account_number}` : 'New customer'}</h2>
          <button type="button" className="modal-x" aria-label="Close" onClick={onClose} disabled={busy}>
            ✕
          </button>
        </div>

        {error && (
          <div className="error" role="alert">
            <span>{error}</span>
            <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
          </div>
        )}

        <form onSubmit={(e) => void handleSubmit(e)}>
          <div className="screen-sub" style={{ marginTop: 0 }}>
            <h4>Identity &amp; contact</h4>
            <div className="screen-form-row">
              <label>
                Account #
                <input
                  type="text"
                  className="screen-input"
                  value={accountNumber}
                  placeholder={isEdit ? '' : 'Auto (e.g. C-1001)'}
                  disabled={busy}
                  onChange={(e) => setAccountNumber(e.target.value)}
                />
              </label>
              <label>
                Customer type
                <select
                  className="screen-input"
                  value={customerType}
                  disabled={busy}
                  onChange={(e) => setCustomerType(e.target.value)}
                >
                  {CUSTOMER_TYPES.map((t) => (
                    <option key={t.value} value={t.value}>{t.label}</option>
                  ))}
                </select>
              </label>
            </div>
            <div className="screen-form-row">
              <label>
                Company name
                <input
                  type="text"
                  className="screen-input"
                  value={companyName}
                  placeholder="Harbourview Painting Ltd."
                  disabled={busy}
                  onChange={(e) => setCompanyName(e.target.value)}
                />
              </label>
              <label>
                First name
                <input
                  type="text"
                  className="screen-input"
                  value={firstName}
                  disabled={busy}
                  onChange={(e) => setFirstName(e.target.value)}
                />
              </label>
              <label>
                Last name
                <input
                  type="text"
                  className="screen-input"
                  value={lastName}
                  disabled={busy}
                  onChange={(e) => setLastName(e.target.value)}
                />
              </label>
            </div>
            <div className="screen-form-row">
              <label>
                Email
                <input
                  type="email"
                  className="screen-input"
                  value={email}
                  placeholder="accounts@example.com"
                  disabled={busy}
                  onChange={(e) => setEmail(e.target.value)}
                />
              </label>
              <label>
                Phone
                <input
                  type="tel"
                  className="screen-input"
                  value={phone}
                  placeholder="604-555-0100"
                  disabled={busy}
                  onChange={(e) => setPhone(e.target.value)}
                />
              </label>
            </div>
          </div>

          <div className="screen-sub">
            <h4>Billing / mailing address</h4>
            <div className="screen-form-row">
              <label>
                Address line 1
                <input
                  type="text"
                  className="screen-input"
                  value={addressLine1}
                  disabled={busy}
                  onChange={(e) => setAddressLine1(e.target.value)}
                />
              </label>
              <label>
                Address line 2
                <input
                  type="text"
                  className="screen-input"
                  value={addressLine2}
                  placeholder="Suite / Unit"
                  disabled={busy}
                  onChange={(e) => setAddressLine2(e.target.value)}
                />
              </label>
            </div>
            <div className="screen-form-row">
              <label>
                City
                <input
                  type="text"
                  className="screen-input"
                  value={city}
                  disabled={busy}
                  onChange={(e) => setCity(e.target.value)}
                />
              </label>
              <label>
                Province / State
                <input
                  type="text"
                  className="screen-input"
                  value={province}
                  placeholder="BC"
                  disabled={busy}
                  onChange={(e) => setProvince(e.target.value)}
                />
              </label>
              <label>
                Postal / ZIP
                <input
                  type="text"
                  className="screen-input"
                  value={postalCode}
                  placeholder="V6B 1A1"
                  disabled={busy}
                  onChange={(e) => setPostalCode(e.target.value)}
                />
              </label>
            </div>
          </div>

          <div className="screen-sub">
            <h4>Pricing, tax &amp; counter rules</h4>
            <div className="screen-form-row">
              <label>
                Price level
                <select
                  className="screen-input"
                  value={priceLevelId}
                  disabled={busy}
                  onChange={(e) => setPriceLevelId(e.target.value)}
                >
                  <option value="">Retail (default)</option>
                  {levels.map((l) => (
                    <option key={l.id} value={l.id}>{l.name}</option>
                  ))}
                </select>
              </label>
              <label>
                Tax-exempt certificate #
                <input
                  type="text"
                  className="screen-input"
                  value={taxExemptCert}
                  disabled={busy}
                  onChange={(e) => setTaxExemptCert(e.target.value)}
                />
              </label>
              <label>
                Tax-exempt expires
                <input
                  type="date"
                  className="screen-input"
                  value={taxExemptExpires}
                  disabled={busy}
                  onChange={(e) => setTaxExemptExpires(e.target.value)}
                />
              </label>
            </div>
            <div className="screen-form-row" style={{ alignItems: 'center' }}>
              <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8, maxWidth: 'none' }}>
                <input
                  type="checkbox"
                  checked={requirePo}
                  disabled={busy}
                  onChange={(e) => setRequirePo(e.target.checked)}
                />
                Require PO # at checkout
              </label>
              <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8, maxWidth: 'none' }}>
                <input
                  type="checkbox"
                  checked={requireJob}
                  disabled={busy}
                  onChange={(e) => setRequireJob(e.target.checked)}
                />
                Require Job name at checkout
              </label>
            </div>
            <div className="screen-form-row">
              <label style={{ maxWidth: 'none' }}>
                Notes
                <textarea
                  className="screen-input"
                  rows={2}
                  value={notes}
                  disabled={busy}
                  onChange={(e) => setNotes(e.target.value)}
                />
              </label>
            </div>
          </div>

          <div className="screen-sub">
            <h4>Charge account &amp; credit controls</h4>
            {!creditEditable && (
              <p className="screen-note">
                Only a Manager or Admin can change charge-account, credit limit, terms, or portal settings.
              </p>
            )}
            <div className="screen-form-row" style={{ alignItems: 'center' }}>
              <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8, maxWidth: 'none' }}>
                <input
                  type="checkbox"
                  checked={chargeEnabled}
                  disabled={busy || !creditEditable}
                  onChange={(e) => setChargeEnabled(e.target.checked)}
                />
                Charge account enabled
              </label>
              <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8, maxWidth: 'none' }}>
                <input
                  type="checkbox"
                  checked={creditHold}
                  disabled={busy || !creditEditable}
                  onChange={(e) => setCreditHold(e.target.checked)}
                />
                Credit hold
              </label>
              <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8, maxWidth: 'none' }}>
                <input
                  type="checkbox"
                  checked={portalEnabled}
                  disabled={busy || !creditEditable}
                  onChange={(e) => setPortalEnabled(e.target.checked)}
                />
                Contractor portal enabled
              </label>
            </div>
            <div className="screen-form-row">
              <label>
                Credit limit ($)
                <input
                  type="text"
                  inputMode="decimal"
                  className="screen-input"
                  value={creditLimit}
                  disabled={busy || !creditEditable}
                  onChange={(e) => setCreditLimit(e.target.value)}
                />
              </label>
              <label>
                Payment terms
                <input
                  type="text"
                  list="customer-payment-terms"
                  className="screen-input"
                  value={paymentTerms}
                  disabled={busy || !creditEditable}
                  onChange={(e) => setPaymentTerms(e.target.value)}
                />
                <datalist id="customer-payment-terms">
                  {COMMON_TERMS.map((t) => (
                    <option key={t} value={t} />
                  ))}
                </datalist>
              </label>
              <label>
                Terms days override
                <input
                  type="number"
                  min={0}
                  max={365}
                  className="screen-input"
                  value={termsDays}
                  placeholder="From terms"
                  disabled={busy || !creditEditable}
                  onChange={(e) => setTermsDays(e.target.value)}
                />
              </label>
              <label>
                Finance rate (% / mo)
                <input
                  type="text"
                  inputMode="decimal"
                  className="screen-input"
                  value={financeRate}
                  placeholder="Default"
                  disabled={busy || !creditEditable}
                  onChange={(e) => setFinanceRate(e.target.value)}
                />
              </label>
            </div>
          </div>

          <div className="modal-foot">
            <button type="button" onClick={onClose} disabled={busy}>
              Cancel
            </button>
            <button type="submit" className="complete-btn" disabled={busy}>
              {busy ? 'Saving…' : isEdit ? 'Save changes' : 'Create customer'}
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}
