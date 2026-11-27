import { useEffect, useMemo, useRef, useState } from 'react'
import {
  TENDER_LABELS,
  cmpDecimal,
  fmtMoney,
  cad,
  type ArCustomerSummary,
  type CustomerSearchResult,
  type TenderKind,
  type TenderRow,
} from '../types'

const TENDERS: { alt: string; letter: string }[] = [
  { alt: 'Alt+1', letter: 'C' },
  { alt: 'Alt+2', letter: 'V' },
  { alt: 'Alt+3', letter: 'M' },
  { alt: 'Alt+4', letter: 'A' },
  { alt: 'Alt+5', letter: 'D' },
  { alt: 'Alt+6', letter: 'E' },
  { alt: 'Alt+7', letter: 'C' },
]
// Tender kinds in Alt+1..7 order (ChargeAccount is the account tender).
const TENDER_KINDS: TenderKind[] = ['Cash', 'Visa', 'Mastercard', 'Amex', 'Debit', 'ETransfer', 'ChargeAccount']

let paySeq = 0
const nextPayKey = () => `p${++paySeq}`

interface PayLine {
  key: string
  kind: TenderKind
  amount: string // applied
  tendered?: string // cash only
}

interface PaymentModalProps {
  totalDue: string // decimal string
  customer: CustomerSearchResult | null
  summary: ArCustomerSummary | null
  onClose: () => void
  onComplete: (tenders: TenderRow[]) => void
  onToast: (msg: string) => void
}

function dec(a: string, b: string, op: '+' | '-'): string {
  const parse4 = (s: string): bigint => {
    const neg = s.trim().startsWith('-')
    const [i, f = ''] = s.trim().replace('-', '').split('.')
    const v = BigInt(i || '0') * 10000n + BigInt((f + '0000').slice(0, 4) || '0')
    return neg ? -v : v
  }
  const v = op === '+' ? parse4(a) + parse4(b) : parse4(a) - parse4(b)
  const sign = v < 0n ? '-' : ''
  const abs = v < 0n ? -v : v
  return `${sign}${(abs / 10000n).toString()}.${(abs % 10000n).toString().padStart(4, '0')}`
}

export default function PaymentModal({ totalDue, customer, summary, onClose, onComplete, onToast }: PaymentModalProps) {
  const [kind, setKind] = useState<TenderKind>('Visa')
  const [amount, setAmount] = useState('')
  const [payments, setPayments] = useState<PayLine[]>([])
  const amountRef = useRef<HTMLInputElement>(null)

  const paidSoFar = useMemo(() => payments.reduce((acc, p) => dec(acc, p.amount, '+'), '0.0000'), [payments])
  const remaining = useMemo(() => dec(totalDue, paidSoFar, '-'), [totalDue, paidSoFar])
  const covered = cmpDecimal(remaining, '0.0000') <= 0
  const cashTenderedTotal = useMemo(
    () => payments.filter((p) => p.kind === 'Cash').reduce((acc, p) => dec(acc, p.tendered ?? p.amount, '+'), '0.0000'),
    [payments],
  )
  // Change due = cash tendered beyond what was still owed after all
  // non-cash payments were applied.
  const changeDue = useMemo(() => {
    const cashApplied = payments
      .filter((p) => p.kind === 'Cash')
      .reduce((acc, p) => dec(acc, p.amount, '+'), '0.0000')
    const nonCashApplied = dec(paidSoFar, cashApplied, '-')
    const owedAfterNonCash = dec(totalDue, nonCashApplied, '-')
    const over = dec(cashTenderedTotal, owedAfterNonCash, '-')
    return cmpDecimal(over, '0.0000') > 0 ? over : '0.0000'
  }, [cashTenderedTotal, totalDue, paidSoFar, payments])
  const chargeSoFar = useMemo(
    () => payments.filter((p) => p.kind === 'ChargeAccount').reduce((acc, p) => dec(acc, p.amount, '+'), '0.0000'),
    [payments],
  )

  const chargeAllowed = !!customer && (summary?.charge_enabled ?? customer.charge_enabled) && !(summary?.credit_hold ?? customer.credit_hold)

  useEffect(() => {
    amountRef.current?.focus()
  }, [kind])

  // Alt+1..7 selects a tender; single letters (not in an input) pick
  // a tender too — C cycles Cash / Charge.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.altKey && /^Digit[1-7]$/.test(e.code)) {
        e.preventDefault()
        const idx = Number(e.code.slice(5)) - 1
        const next = TENDER_KINDS[idx]
        if (next === 'ChargeAccount' && !chargeAllowed) {
          onToast('Charge to account needs a customer with an open account.')
          return
        }
        setKind(next)
        return
      }
      const target = e.target as HTMLElement
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.tagName === 'SELECT')) return
      if (e.altKey || e.ctrlKey || e.metaKey) return
      const letter = e.key.toUpperCase()
      if (letter === 'V') setKind('Visa')
      else if (letter === 'M') setKind('Mastercard')
      else if (letter === 'A') setKind('Amex')
      else if (letter === 'D') setKind('Debit')
      else if (letter === 'E') setKind('ETransfer')
      else if (letter === 'C') {
        setKind((cur) => {
          if (cur === 'Cash') return chargeAllowed ? 'ChargeAccount' : 'Cash'
          if (cur === 'ChargeAccount') return 'Cash'
          return 'Cash'
        })
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [chargeAllowed, onToast])

  const defaultAmount = remaining
  const amountValue = amount === '' ? defaultAmount : amount

  function addPayment() {
    const raw = (amount === '' ? remaining : amount).trim()
    if (!raw || cmpDecimal(raw, '0.0000') <= 0) {
      onToast('Enter an amount greater than $0.00.')
      return
    }
    if (kind === 'ChargeAccount') {
      if (!chargeAllowed) {
        onToast('Charge to account needs a customer with an open account.')
        return
      }
      const limit = summary?.credit_limit ?? customer?.credit_limit
      const balance = summary?.balance
      if (limit && balance) {
        const projected = dec(dec(balance.amount, chargeSoFar, '+'), raw, '+')
        if (cmpDecimal(projected, limit.amount) > 0) {
          onToast(
            `Charge of ${fmtMoney(cad(raw))} would take the balance to ${fmtMoney(cad(projected))}, over the ${fmtMoney(limit)} credit limit — manager approval needed.`,
          )
          return
        }
      }
      const applied = cmpDecimal(raw, remaining) > 0 ? remaining : raw
      setPayments((prev) => [...prev, { key: nextPayKey(), kind, amount: applied }])
    } else if (kind === 'Cash') {
      const tendered = raw
      const applied = cmpDecimal(tendered, remaining) > 0 ? remaining : tendered
      setPayments((prev) => [...prev, { key: nextPayKey(), kind, amount: applied, tendered }])
    } else {
      const applied = cmpDecimal(raw, remaining) > 0 ? remaining : raw
      setPayments((prev) => [...prev, { key: nextPayKey(), kind, amount: applied }])
    }
    setAmount('')
    amountRef.current?.focus()
  }

  const cardSuffix = (k: TenderKind) =>
    k === 'Visa' || k === 'Mastercard' || k === 'Amex' || k === 'Debit' ? ' · Terminal 1' : ''

  return (
    <div className="modal-overlay" role="dialog" aria-modal="true" aria-label="Payment">
      <div className="modal payment-modal">
        <div className="modal-head">
          <h2>Payment</h2>
          <span className="total-chip">TOTAL {fmtMoney(cad(totalDue))}</span>
          <button type="button" className="modal-x" onClick={onClose} aria-label="Close payment">✕</button>
        </div>
        <div className="pay-summary">
          <div><span>Total due</span><strong>{fmtMoney(cad(totalDue))}</strong></div>
          <div><span>Paid so far</span><strong>{fmtMoney(cad(paidSoFar))}</strong></div>
          <div className={covered ? 'covered' : ''}>
            <span>Balance remaining</span><strong>{fmtMoney(cad(remaining))}</strong>
          </div>
        </div>

        <div className="tender-grid" role="group" aria-label="Tender type">
          {TENDERS.map((t, i) => {
            const k = TENDER_KINDS[i]
            const disabled = k === 'ChargeAccount' && !chargeAllowed
            return (
              <button
                key={k}
                type="button"
                className={`tender-btn${kind === k ? ' selected' : ''}`}
                aria-pressed={kind === k}
                disabled={disabled}
                title={disabled ? 'Needs a customer with an open account' : `${t.alt} / ${t.letter}`}
                onClick={() => setKind(k)}
              >
                {TENDER_LABELS[k]}
                {cardSuffix(k)}
                <span className="tender-key">{t.alt}</span>
              </button>
            )
          })}
        </div>

        {kind === 'Cash' && (
          <div className="cash-quick">
            {['20', '50', '100'].map((v) => (
              <button key={v} type="button" onClick={() => setAmount(`${v}.00`)}>${v}</button>
            ))}
            <button type="button" onClick={() => setAmount(remaining)}>Exact</button>
          </div>
        )}

        <label className="pay-amount">
          {kind === 'Cash' ? 'Cash tendered' : `Amount — ${TENDER_LABELS[kind]}`}
          <input
            ref={amountRef}
            value={amount}
            onChange={(e) => setAmount(e.target.value.replace(/[^0-9.]/g, ''))}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                addPayment()
              }
            }}
            placeholder={remaining}
            inputMode="decimal"
          />
        </label>
        <button type="button" className="add-payment" onClick={addPayment} disabled={covered}>
          Add payment
        </button>

        {cmpDecimal(changeDue, '0.0000') > 0 && (
          <div className="change-row"><span>Change due</span><strong>{fmtMoney(cad(changeDue))}</strong></div>
        )}

        {payments.length > 0 && (
          <ul className="pay-list">
            {payments.map((p) => (
              <li key={p.key}>
                <span>{TENDER_LABELS[p.kind]}{cardSuffix(p.kind)}</span>
                <span>
                  {fmtMoney(cad(p.amount))}
                  {p.tendered && cmpDecimal(p.tendered, p.amount) > 0 && (
                    <small> tendered {fmtMoney(cad(p.tendered))}</small>
                  )}
                </span>
                <button
                  type="button"
                  aria-label={`Remove ${TENDER_LABELS[p.kind]} payment`}
                  onClick={() => setPayments((prev) => prev.filter((x) => x.key !== p.key))}
                >
                  ✕
                </button>
              </li>
            ))}
          </ul>
        )}

        <p className="hint">Split the sale across tenders. Keys: Alt+1…Alt+7, or the first letter of a tender (C cycles Cash / Charge).</p>

        <div className="modal-foot">
          <button type="button" onClick={onClose}>Back to sale</button>
          <button
            type="button"
            className="complete-btn"
            disabled={!covered}
            onClick={() =>
              onComplete(
                payments.map((p) => ({ key: nextPayKey(), kind: p.kind, amount: p.amount, tendered: p.tendered })),
              )
            }
          >
            Complete sale · {fmtMoney(cad(totalDue))}
          </button>
        </div>
      </div>
    </div>
  )
}
