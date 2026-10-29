# Selling deskMonk Pro with Paddle

Paddle is the seller of record: it charges the card, handles sales tax and VAT, sends receipts, and pays you out. deskMonk's own part runs on deskmonk.app:

- `/buy`: Monthly/Yearly cards with local prices from Paddle, plus an email field. "Continue to payment" opens Paddle's overlay with an order reference.
- `/welcome?ref=…`: the page after payment. It waits for the webhook, then shows the license key.
- `/account`: the buyer enters a key and sees the plan, the end date and the number of computers. Its "Manage subscription" button opens Paddle's portal.
- `/api/paddle` receives Paddle's webhooks and creates the license.
- `/admin` shows the subscriptions.

## Which Paddle account

**Recommended: the Paddle account you already have for bubbleTranslate.** It is already verified, and its payouts are already set up. deskMonk only needs:

- a second approved website (deskmonk.app)
- a new product

One thing to know: Paddle sends every webhook event to every notification destination, so each app also receives the other app's sales. Both apps need to handle that:

- **deskMonk** ignores subscriptions that aren't for its own two prices (`planOf` in `src/license.ts`).
- **bubbleTranslate** currently stores every subscription it receives in its Paddle mirror. deskMonk sales would therefore show up in its admin and CSV until it gets the same price-id check. That change is not done.

The alternative is a separate Paddle account for deskMonk. It means the full verification again (identity, business, website review), which takes days.

## 1. Get deskmonk.app approved (live account)

Go to Paddle → **Checkout → Website approval**, add `https://deskmonk.app`, and submit. Paddle's reviewers check that the site has:

| They look for | Where it is |
|---|---|
| What the product is | https://deskmonk.app |
| Prices ($2.50 a month, $25 a year) | https://deskmonk.app/pricing |
| Terms of service, with the seller name matching the Paddle account (Mehmet Bilen) | https://deskmonk.app/terms |
| Privacy policy | https://deskmonk.app/privacy |
| Refund policy (14 days) | https://deskmonk.app/refunds |
| A way to contact you | support@deskmonk.app (forwards to your Gmail) |

Approval usually takes a few working days, and Paddle may email you questions. Then set **Checkout → Checkout settings → Default payment link** to `https://deskmonk.app/buy`.

## 2. Catalog

Go to **Catalog → Products → New product**:

- Name: "deskMonk Pro"
- Tax category: standard digital goods / software

Add two recurring prices:

- **Monthly:** $2.50, billed every 1 month
- **Yearly:** $25.00, billed every 1 year

Copy both price IDs (`pri_…`).

## 3. Keys

Go to **Developer tools → Authentication**:

- **Client-side token:** used by the checkout page.
- **API key:** its permissions must allow reading customers and subscriptions, and creating customer portal sessions. It is used to look up the buyer's email and for the "Manage subscription" link.

## 4. Webhook

Go to **Developer tools → Notifications → New destination**:

- URL: `https://deskmonk.app/api/paddle`
- Events: all `subscription.*` events

Copy the destination's **secret key**.

## 5. Put it in the server

In `worker/wrangler.toml`, set `PADDLE_PRICE_MONTHLY` and `PADDLE_PRICE_YEARLY`, and set `PADDLE_ENV = "production"` for the live account. Then run, in `worker/`:

```sh
npx wrangler secret put PADDLE_CLIENT_TOKEN
npx wrangler secret put PADDLE_API_KEY
npx wrangler secret put PADDLE_WEBHOOK_SECRET
npx wrangler deploy
```

`https://deskmonk.app/buy?plan=monthly` now opens Paddle's checkout instead of "Checkout opens soon". The admin top bar shows **Paddle production**.

## 6. Turn on the free limits in the apps

The limits and the Pro button only exist in builds that know the license server. Set a GitHub repository variable `DESKMONK_LICENSE_URL` to `https://deskmonk.app`; CI builds from then on include it. Do this only after checkout works: without it, users would hit limits they can't buy their way out of.

## Test first (sandbox)

1. Make a free account at https://sandbox-vendors.paddle.com. It is separate from the live account and needs no approval.
2. Do steps 2–5 there, keeping `PADDLE_ENV = "sandbox"`.
3. In a deskMonk build that has `DESKMONK_LICENSE_URL`, press ⭐ Get Pro → Monthly and pay with test card `4242 4242 4242 4242` (any future date, any CVC).
4. Check that:
   - the app turns Pro within seconds
   - /admin shows the subscription with the buyer's email
   - ⭐ Pro → Manage subscription opens Paddle's portal
   - cancelling there turns the app back to Free after the paid period

Sandbox and live have different keys, prices and secrets; switch all of them together.
