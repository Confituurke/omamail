.pragma library

// Whether the LNemail renewal control belongs in the header: looking at
// one LNemail mailbox on its own, or at every mailbox at once with at
// least one LNemail account among them. Neither the single-account
// question nor the unified one alone is enough — a header built for
// whichever mailbox happens to be current would hide the control the
// moment "All mail" was chosen with an LNemail account still connected.
function hasAccountInView(service) {
  if (!service) return false
  if (service.unified) {
    var accounts = (service.accountList && service.accountList.accounts) ? service.accountList.accounts : []
    return accounts.some(function(account) { return account.provider === "lnemail" })
  }
  return service.providerId === "lnemail"
}

// Looking at exactly one LNemail mailbox is the one case worth naming its
// own status at a glance; "All mail" may hold more than one, and the popup
// itself is where each one's own expiry is worth reading.
function currentAuth(service) {
  return (service && !service.unified && service.providerId === "lnemail") ? service.auth : null
}

function tooltip(auth) {
  if (!auth || !auth.statusChecked) return "LNemail renewal"
  if (auth.isExpired) return "LNemail mailbox expired — renew"
  if (auth.daysUntilExpiry < 0) return "LNemail renewal"
  return "LNemail renews in " + auth.daysUntilExpiry + (auth.daysUntilExpiry === 1 ? " day" : " days")
}

function urgent(auth) {
  return !!auth && auth.statusChecked
    && (auth.isExpired || (auth.daysUntilExpiry >= 0 && auth.daysUntilExpiry <= 30))
}
