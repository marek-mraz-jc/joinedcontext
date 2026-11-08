#![forbid(unsafe_code)]
//! Internationalization bundles for `en`, `sk`, `cs`, `de`, and `hu` (W7).
//! Message keys and texts come directly from `charts/theme/files/login__messages__messages_{en,sk,cs,de}.properties`,
//! plus Hungarian (`hu`) and field labels/hints so no Askama template holds a literal user-facing string.

use std::collections::BTreeMap;

pub const SUPPORTED_LOCALES: [&str; 5] = ["en", "sk", "cs", "de", "hu"];

pub struct Bundle {
    pub locale: &'static str,
    messages: BTreeMap<&'static str, &'static str>,
}

const EN_PROPERTIES: &str = r#"
jcSignInTo=Sign in to {0} · {1}
jcSignInToApp=Sign in to {0}
jcBackTo=Back to {0}
jcOrganizationFallback=this platform
jcErrorNextStep=Close this page and open the app again from its link. If this keeps happening, tell your administrator the time it happened.
jcPageExpiredExplained=This page was open too long to be sent. You can pick up where you left off, or start signing in again.
jcPageExpiredContinue=Continue where you left off
jcPageExpiredRestart=Start signing in again
jcClientPortal=the Portal
jcClientCatalogue=the open data catalogue
jcClientParticipation=the participation platform
jcClientForge=the code forge
jcClientMonitoring=monitoring
loginAccountTitle=Sign in
doLogIn=Sign in
doForgotPassword=Forgot your password?
errorTitle=Something went wrong
invalidUserMessage=The e-mail or password is not right. Check both and try again.
missingUsernameMessage=Enter your e-mail or username.
missingPasswordMessage=Enter your password.
loginTimeout=Signing in took too long, so it starts again. Enter your details once more.
expiredCodeMessage=Signing in took too long. Sign in again.
expiredActionMessage=This link was used or has expired. Sign in to continue.
expiredActionTokenNoSessionMessage=This link has expired. Ask for a new one.
expiredActionTokenSessionExistsMessage=This link has expired. Ask for a new one.
invalidCodeMessage=Your sign-in could not be finished. Open the app again and sign in from there.
cookieNotFoundMessage=Your browser did not keep the sign-in cookie. Allow cookies for this site, then open the app again.
pageExpiredTitle=This page has expired
emailForgotTitle=Reset your password
emailInstruction=Enter your e-mail or username. We send you a link to choose a new password.
emailInstructionUsername=Enter your username. We send you a link to choose a new password.
emailSentMessage=If an account matches, an e-mail with a link is on its way. Check your inbox.
backToLogin=Back to sign in
updatePasswordTitle=Choose a new password
emailVerifyTitle=Confirm your e-mail
accountUpdatedMessage=Your account is updated.
backToApplication=Back to the application
proceedWithAction=Continue
logoutConfirmTitle=Sign out
logoutConfirmHeader=Do you want to sign out? You are signed out of every app of this organization.
doLogout=Sign out
invalidParameterMessage=The app asked to sign you in in a way this platform does not accept ({0}). Open the app again from its usual link; if this keeps happening, tell your administrator.
usernameOrEmailLabel=E-mail or username
usernameOrEmailHint=For example, name@organization.org
passwordLabel=Password
passwordHint=At least 12 characters with uppercase, lowercase and a digit
otpLabel=One-time code
otpHint=6-digit code from your authenticator app (HMAC-SHA256)
otpEnrolTitle=Set up two-factor authentication
otpEnrolInstruction=Add this key to your authenticator app (time-based, {1}, {2} digits, {3} s), then enter the code it shows: {0}
recoveryCodeLabel=Recovery code
recoveryCodeHint=Enter one of your 10 single-use backup codes
orgChoiceTitle=Choose your organization
orgChoiceLabel=Organization
orgChoiceHint=Select the organization this session acts for
consentTitle=Grant access
consentInstruction=This application asks to use your account with the requested scopes.
termsTitle=Terms and conditions
termsInstruction=Accept the terms of service to continue signing in.
otpDeviceLabel=Device name
otpDeviceHint=Optional, for example Work phone. It names this authenticator on your account page.
doSubmit=Submit
doCancel=Cancel
passwordNewLabel=New password
passwordConfirmLabel=Confirm password
passwordConfirmHint=Type the new password once more
notMatchPasswordMessage=The two passwords do not match. Type the same password in both fields.
logoutOtherSessions=Sign out from other devices
logoutOtherSessionsHint=Every other browser and app signed in to your account has to sign in again.
reauthenticate=Sign in again to continue. This keeps your account safe before a security change.
accountTitle=Account
accountPersonalInfo=Personal info
accountPersonalInfoIntro=Manage your basic information.
accountSecurity=Account security
accountSigningIn=Signing in
accountSigningInIntro=Configure ways to sign in.
accountDeviceActivity=Device activity
accountDeviceActivityIntro=See where you are signed in and sign out of any device.
accountUsername=Username
accountEmail=Email
accountEmailHint=For example, name@organization.org. A new address has to be confirmed again.
accountEmailUnverified=This address is not confirmed yet.
accountFirstName=First name
accountLastName=Last name
accountSave=Save
accountSaved=Your account has been updated.
accountUpdate=Update
accountCreated=Created
accountBasicAuth=Basic authentication
accountPasswordIntro=Sign in by entering your password.
accountPasswordNone=No password is set.
accountTwoFactor=Two-factor authentication
accountOtp=Authenticator application
accountOtpIntro=Enter a verification code from your authenticator application.
accountOtpSetUp=Set up authenticator application
accountOtpNone=Authenticator application is not set up.
accountOtpRequiredHint=Your organization requires an authenticator application, so the last one cannot be removed.
accountOtpRequired=Your organization requires an authenticator application. Set up another one before you remove this one.
accountPasskeys=Passkeys
accountPasskeysIntro=Sign in with a passkey.
accountRemove=Remove
accountRemoveConfirm=Remove {0}? You can no longer sign in with it.
accountRemoveConfirmButton=Yes, remove
accountRemoved=The credential is removed.
accountCredentialNotRemovable=This sign-in method cannot be removed.
accountSignedInDevices=Signed in devices
accountCurrentSession=Current session
accountStarted=Started
accountLastAccess=Last accessed
accountExpires=Expires
accountClients=Applications
accountSignOut=Sign out
accountSignOutAll=Sign out all devices
accountSignOutAllConfirm=This signs out every device, this one included.
accountSignedOut=The device is signed out.
accountSignOutLink=Sign out
accountLoading=Loading…
accountError=Something went wrong. Reload the page and try again.
accountActionSuccess=Your change is saved.
accountActionCancelled=Nothing was changed.
accountMenu=Menu
accountSkip=Skip to content
accountInvalidRequest=The change could not be read. Reload the page and try again.
accountFieldReadOnly=This field cannot be changed here.
accountFieldTooLong=This value is too long (at most 255 characters).
accountFieldInvalid=This value is not valid.
accountEmailRequired=Enter your e-mail address.
accountEmailInvalid=Enter a valid e-mail address, for example name@organization.org.
accountEmailInUse=Another account already uses this e-mail address.
accountAttributeReadOnly=This attribute is managed by your administrator and cannot be changed here.
"#;

const SK_PROPERTIES: &str = r#"
jcSignInTo=Prihlásenie: {0} · {1}
jcSignInToApp=Prihlásenie: {0}
jcBackTo=Späť: {0}
jcOrganizationFallback=táto platforma
jcErrorNextStep=Zatvorte túto stránku a otvorte aplikáciu znova cez jej odkaz. Ak sa to opakuje, povedzte správcovi, kedy sa to stalo.
jcPageExpiredExplained=Stránka bola otvorená príliš dlho a nedá sa odoslať. Môžete pokračovať tam, kde ste skončili, alebo sa začať prihlasovať odznova.
jcPageExpiredContinue=Pokračovať, kde ste prestali
jcPageExpiredRestart=Začať prihlásenie odznova
jcClientPortal=Portál
jcClientCatalogue=Katalóg otvorených dát
jcClientParticipation=Platforma participácie
jcClientForge=Repozitár kódu
jcClientMonitoring=Monitorovanie
loginAccountTitle=Prihlásiť sa
doLogIn=Prihlásiť sa
doForgotPassword=Zabudli ste heslo?
errorTitle=Niečo sa pokazilo
invalidUserMessage=E-mail alebo heslo nie je správne. Skontrolujte oboje a skúste to znova.
missingUsernameMessage=Zadajte e-mail alebo používateľské meno.
missingPasswordMessage=Zadajte heslo.
loginTimeout=Prihlasovanie trvalo príliš dlho, preto začína odznova. Zadajte údaje ešte raz.
expiredCodeMessage=Prihlasovanie trvalo príliš dlho. Prihláste sa znova.
expiredActionMessage=Tento odkaz už bol použitý alebo mu vypršala platnosť. Prihláste sa a pokračujte.
expiredActionTokenNoSessionMessage=Platnosť odkazu vypršala. Požiadajte o nový.
expiredActionTokenSessionExistsMessage=Platnosť odkazu vypršala. Požiadajte o nový.
invalidCodeMessage=Prihlásenie sa nepodarilo dokončiť. Otvorte aplikáciu znova a prihláste sa z nej.
cookieNotFoundMessage=Prehliadač si neuložil prihlasovací cookie. Povoľte cookies pre túto stránku a otvorte aplikáciu znova.
pageExpiredTitle=Platnosť stránky vypršala
emailForgotTitle=Obnovenie hesla
emailInstruction=Zadajte e-mail alebo používateľské meno. Pošleme vám odkaz na nové heslo.
emailInstructionUsername=Zadajte používateľské meno. Pošleme vám odkaz na nové heslo.
emailSentMessage=Ak účet existuje, e-mail s odkazom je na ceste. Skontrolujte si schránku.
backToLogin=Späť na prihlásenie
updatePasswordTitle=Zvoľte si nové heslo
emailVerifyTitle=Potvrďte e-mail
accountUpdatedMessage=Váš účet je aktualizovaný.
backToApplication=Späť do aplikácie
proceedWithAction=Pokračovať
logoutConfirmTitle=Odhlásiť sa
logoutConfirmHeader=Chcete sa odhlásiť? Odhlásite sa zo všetkých aplikácií tejto organizácie.
doLogout=Odhlásiť sa
invalidParameterMessage=Aplikácia požiadala o prihlásenie spôsobom, ktorý táto platforma neprijíma ({0}). Otvorte aplikáciu znova cez jej obvyklý odkaz; ak sa to opakuje, povedzte to správcovi.
usernameOrEmailLabel=E-mail alebo používateľské meno
usernameOrEmailHint=Napríklad meno@organizacia.sk
passwordLabel=Heslo
passwordHint=Aspoň 12 znakov s veľkým a malým písmenom a číslicou
otpLabel=Jednorazový kód
otpHint=6-miestny kód z overovacej aplikácie (HMAC-SHA256)
otpEnrolTitle=Nastavenie dvojfaktorového overenia
otpEnrolInstruction=Pridajte tento kľúč do autentifikačnej aplikácie (časový, {1}, {2} číslic, {3} s) a zadajte kód, ktorý zobrazí: {0}
recoveryCodeLabel=Záložný kód
recoveryCodeHint=Zadajte jeden z 10 jednorazových záložných kódov
orgChoiceTitle=Vyberte organizáciu
orgChoiceLabel=Organizácia
orgChoiceHint=Vyberte organizáciu, za ktorú táto relácia koná
consentTitle=Udeliť prístup
consentInstruction=Táto aplikácia žiada o použitie vášho účtu s požadovanými oprávneniami.
termsTitle=Podmienky používania
termsInstruction=Pre pokračovanie v prihlásení prijmite podmienky používania.
otpDeviceLabel=Názov zariadenia
otpDeviceHint=Nepovinné, napríklad Pracovný telefón. Týmto názvom sa overovacia aplikácia zobrazí na stránke vášho účtu.
doSubmit=Odoslať
doCancel=Zrušiť
passwordNewLabel=Nové heslo
passwordConfirmLabel=Potvrdenie hesla
passwordConfirmHint=Zadajte nové heslo ešte raz
notMatchPasswordMessage=Heslá sa nezhodujú. Do oboch polí zadajte rovnaké heslo.
logoutOtherSessions=Odhlásiť z ostatných zariadení
logoutOtherSessionsHint=Každý iný prehliadač a aplikácia prihlásená k vášmu účtu sa bude musieť prihlásiť znova.
reauthenticate=Na pokračovanie sa prihláste znova. Pred zmenou zabezpečenia tak chránime váš účet.
accountTitle=Účet
accountPersonalInfo=Osobné údaje
accountPersonalInfoIntro=Spravujte svoje základné údaje.
accountSecurity=Zabezpečenie účtu
accountSigningIn=Prihlasovanie
accountSigningInIntro=Nastavte spôsoby prihlásenia.
accountDeviceActivity=Aktivita zariadení
accountDeviceActivityIntro=Pozrite si, kde ste prihlásení, a odhláste sa z ľubovoľného zariadenia.
accountUsername=Používateľské meno
accountEmail=E-mail
accountEmailHint=Napríklad meno@organizacia.sk. Novú adresu treba znova potvrdiť.
accountEmailUnverified=Táto adresa ešte nie je potvrdená.
accountFirstName=Meno
accountLastName=Priezvisko
accountSave=Uložiť
accountSaved=Váš účet bol aktualizovaný.
accountUpdate=Zmeniť
accountCreated=Vytvorené
accountBasicAuth=Základné overenie
accountPasswordIntro=Prihlasujte sa zadaním hesla.
accountPasswordNone=Heslo nie je nastavené.
accountTwoFactor=Dvojfaktorové overenie
accountOtp=Overovacia aplikácia
accountOtpIntro=Zadajte overovací kód z overovacej aplikácie.
accountOtpSetUp=Nastaviť overovaciu aplikáciu
accountOtpNone=Overovacia aplikácia nie je nastavená.
accountOtpRequiredHint=Vaša organizácia vyžaduje overovaciu aplikáciu, preto poslednú nemožno odstrániť.
accountOtpRequired=Vaša organizácia vyžaduje overovaciu aplikáciu. Pred odstránením tejto si nastavte ďalšiu.
accountPasskeys=Prístupové kľúče
accountPasskeysIntro=Prihlasujte sa prístupovým kľúčom.
accountRemove=Odstrániť
accountRemoveConfirm=Odstrániť {0}? Už sa ním nebudete môcť prihlásiť.
accountRemoveConfirmButton=Áno, odstrániť
accountRemoved=Prihlasovací údaj bol odstránený.
accountCredentialNotRemovable=Tento spôsob prihlásenia nemožno odstrániť.
accountSignedInDevices=Prihlásené zariadenia
accountCurrentSession=Aktuálna relácia
accountStarted=Začiatok
accountLastAccess=Posledný prístup
accountExpires=Platnosť do
accountClients=Aplikácie
accountSignOut=Odhlásiť
accountSignOutAll=Odhlásiť všetky zariadenia
accountSignOutAllConfirm=Odhlásia sa všetky zariadenia vrátane tohto.
accountSignedOut=Zariadenie bolo odhlásené.
accountSignOutLink=Odhlásiť sa
accountLoading=Načítava sa…
accountError=Niečo sa pokazilo. Obnovte stránku a skúste to znova.
accountActionSuccess=Zmena je uložená.
accountActionCancelled=Nič sa nezmenilo.
accountMenu=Ponuka
accountSkip=Preskočiť na obsah
accountInvalidRequest=Zmenu sa nepodarilo prečítať. Obnovte stránku a skúste to znova.
accountFieldReadOnly=Toto pole tu nemožno zmeniť.
accountFieldTooLong=Hodnota je príliš dlhá (najviac 255 znakov).
accountFieldInvalid=Hodnota nie je platná.
accountEmailRequired=Zadajte svoju e-mailovú adresu.
accountEmailInvalid=Zadajte platnú e-mailovú adresu, napríklad meno@organizacia.sk.
accountEmailInUse=Túto e-mailovú adresu už používa iný účet.
accountAttributeReadOnly=Tento atribút spravuje váš správca a tu ho nemožno zmeniť.
"#;

const CS_PROPERTIES: &str = r#"
jcSignInTo=Přihlášení: {0} · {1}
jcSignInToApp=Přihlášení: {0}
jcBackTo=Zpět: {0}
jcOrganizationFallback=tato platforma
jcErrorNextStep=Zavřete tuto stránku a otevřete aplikaci znovu přes její odkaz. Pokud se to opakuje, řekněte správci, kdy se to stalo.
jcPageExpiredExplained=Stránka byla otevřená příliš dlouho a nelze ji odeslat. Můžete pokračovat tam, kde jste skončili, nebo se začít přihlašovat znovu.
jcPageExpiredContinue=Pokračovat, kde jste skončili
jcPageExpiredRestart=Začít přihlášení znovu
jcClientPortal=Portál
jcClientCatalogue=Katalog otevřených dat
jcClientParticipation=Platforma participace
jcClientForge=Repozitář kódu
jcClientMonitoring=Monitorování
loginAccountTitle=Přihlásit se
doLogIn=Přihlásit se
doForgotPassword=Zapomněli jste heslo?
errorTitle=Něco se pokazilo
invalidUserMessage=E-mail nebo heslo není správné. Zkontrolujte obojí a zkuste to znovu.
missingUsernameMessage=Zadejte e-mail nebo uživatelské jméno.
missingPasswordMessage=Zadejte heslo.
loginTimeout=Přihlašování trvalo příliš dlouho, proto začíná znovu. Zadejte údaje ještě jednou.
expiredCodeMessage=Přihlašování trvalo příliš dlouho. Přihlaste se znovu.
expiredActionMessage=Tento odkaz už byl použit nebo mu vypršela platnost. Přihlaste se a pokračujte.
expiredActionTokenNoSessionMessage=Platnost odkazu vypršela. Požádejte o nový.
expiredActionTokenSessionExistsMessage=Platnost odkazu vypršela. Požádejte o nový.
invalidCodeMessage=Přihlášení se nepodařilo dokončit. Otevřete aplikaci znovu a přihlaste se z ní.
cookieNotFoundMessage=Prohlížeč si neuložil přihlašovací cookie. Povolte cookies pro tuto stránku a otevřete aplikaci znovu.
pageExpiredTitle=Platnost stránky vypršela
emailForgotTitle=Obnovení hesla
emailInstruction=Zadejte e-mail nebo uživatelské jméno. Pošleme vám odkaz na nové heslo.
emailInstructionUsername=Zadejte uživatelské jméno. Pošleme vám odkaz na nové heslo.
emailSentMessage=Pokud účet existuje, e-mail s odkazem je na cestě. Zkontrolujte si schránku.
backToLogin=Zpět na přihlášení
updatePasswordTitle=Zvolte si nové heslo
emailVerifyTitle=Potvrďte e-mail
accountUpdatedMessage=Váš účet je aktualizován.
backToApplication=Zpět do aplikace
proceedWithAction=Pokračovat
logoutConfirmTitle=Odhlásit se
logoutConfirmHeader=Chcete se odhlásit? Odhlásíte se ze všech aplikací této organizace.
doLogout=Odhlásit se
invalidParameterMessage=Aplikace požádala o přihlášení způsobem, který tato platforma nepřijímá ({0}). Otevřete aplikaci znovu přes její obvyklý odkaz; pokud se to opakuje, řekněte to správci.
usernameOrEmailLabel=E-mail nebo uživatelské jméno
usernameOrEmailHint=Například jmeno@organizace.cz
passwordLabel=Heslo
passwordHint=Alespoň 12 znaků s velkým a malým písmenem a číslicí
otpLabel=Jednorázový kód
otpHint=6místný kód z ověřovací aplikace (HMAC-SHA256)
otpEnrolTitle=Nastavení dvoufaktorového ověření
otpEnrolInstruction=Přidejte tento klíč do ověřovací aplikace (časový, {1}, {2} číslic, {3} s) a zadejte kód, který zobrazí: {0}
recoveryCodeLabel=Záložní kód
recoveryCodeHint=Zadejte jeden z 10 jednorázových záložních kódů
orgChoiceTitle=Vyberte organizaci
orgChoiceLabel=Organizace
orgChoiceHint=Vyberte organizaci, za kterou tato relace jedná
consentTitle=Udělit přístup
consentInstruction=Tato aplikace žádá o použití vašeho účtu s požadovanými oprávněními.
termsTitle=Podmínky použití
termsInstruction=Pro pokračování v přihlášení přijměte podmínky použití.
otpDeviceLabel=Název zařízení
otpDeviceHint=Nepovinné, například Pracovní telefon. Tímto názvem se ověřovací aplikace zobrazí na stránce vašeho účtu.
doSubmit=Odeslat
doCancel=Zrušit
passwordNewLabel=Nové heslo
passwordConfirmLabel=Potvrzení hesla
passwordConfirmHint=Zadejte nové heslo ještě jednou
notMatchPasswordMessage=Hesla se neshodují. Do obou polí zadejte stejné heslo.
logoutOtherSessions=Odhlásit z ostatních zařízení
logoutOtherSessionsHint=Každý jiný prohlížeč a aplikace přihlášená k vašemu účtu se bude muset přihlásit znovu.
reauthenticate=Pro pokračování se přihlaste znovu. Před změnou zabezpečení tak chráníme váš účet.
accountTitle=Účet
accountPersonalInfo=Osobní údaje
accountPersonalInfoIntro=Spravujte své základní údaje.
accountSecurity=Zabezpečení účtu
accountSigningIn=Přihlašování
accountSigningInIntro=Nastavte způsoby přihlášení.
accountDeviceActivity=Aktivita zařízení
accountDeviceActivityIntro=Podívejte se, kde jste přihlášeni, a odhlaste se z libovolného zařízení.
accountUsername=Uživatelské jméno
accountEmail=E-mail
accountEmailHint=Například jmeno@organizace.cz. Novou adresu je třeba znovu potvrdit.
accountEmailUnverified=Tato adresa ještě není potvrzena.
accountFirstName=Jméno
accountLastName=Příjmení
accountSave=Uložit
accountSaved=Váš účet byl aktualizován.
accountUpdate=Změnit
accountCreated=Vytvořeno
accountBasicAuth=Základní ověření
accountPasswordIntro=Přihlašujte se zadáním hesla.
accountPasswordNone=Heslo není nastaveno.
accountTwoFactor=Dvoufaktorové ověření
accountOtp=Ověřovací aplikace
accountOtpIntro=Zadejte ověřovací kód z ověřovací aplikace.
accountOtpSetUp=Nastavit ověřovací aplikaci
accountOtpNone=Ověřovací aplikace není nastavena.
accountOtpRequiredHint=Vaše organizace vyžaduje ověřovací aplikaci, proto poslední nelze odstranit.
accountOtpRequired=Vaše organizace vyžaduje ověřovací aplikaci. Před odstraněním této si nastavte další.
accountPasskeys=Přístupové klíče
accountPasskeysIntro=Přihlašujte se přístupovým klíčem.
accountRemove=Odstranit
accountRemoveConfirm=Odstranit {0}? Už se jím nebudete moci přihlásit.
accountRemoveConfirmButton=Ano, odstranit
accountRemoved=Přihlašovací údaj byl odstraněn.
accountCredentialNotRemovable=Tento způsob přihlášení nelze odstranit.
accountSignedInDevices=Přihlášená zařízení
accountCurrentSession=Aktuální relace
accountStarted=Začátek
accountLastAccess=Poslední přístup
accountExpires=Platnost do
accountClients=Aplikace
accountSignOut=Odhlásit
accountSignOutAll=Odhlásit všechna zařízení
accountSignOutAllConfirm=Odhlásí se všechna zařízení včetně tohoto.
accountSignedOut=Zařízení bylo odhlášeno.
accountSignOutLink=Odhlásit se
accountLoading=Načítá se…
accountError=Něco se pokazilo. Obnovte stránku a zkuste to znovu.
accountActionSuccess=Změna je uložena.
accountActionCancelled=Nic se nezměnilo.
accountMenu=Nabídka
accountSkip=Přeskočit na obsah
accountInvalidRequest=Změnu se nepodařilo přečíst. Obnovte stránku a zkuste to znovu.
accountFieldReadOnly=Toto pole zde nelze změnit.
accountFieldTooLong=Hodnota je příliš dlouhá (nejvýše 255 znaků).
accountFieldInvalid=Hodnota není platná.
accountEmailRequired=Zadejte svou e-mailovou adresu.
accountEmailInvalid=Zadejte platnou e-mailovou adresu, například jmeno@organizace.cz.
accountEmailInUse=Tuto e-mailovou adresu už používá jiný účet.
accountAttributeReadOnly=Tento atribut spravuje váš správce a zde jej nelze změnit.
"#;

const DE_PROPERTIES: &str = r#"
jcSignInTo=Anmelden: {0} · {1}
jcSignInToApp=Anmelden: {0}
jcBackTo=Zurück: {0}
jcOrganizationFallback=diese Plattform
jcErrorNextStep=Schließen Sie diese Seite und öffnen Sie die Anwendung erneut über ihren Link. Wenn das wiederholt passiert, nennen Sie Ihrer Administration die Uhrzeit.
jcPageExpiredExplained=Diese Seite war zu lange geöffnet, um sie abzusenden. Sie können dort weitermachen, wo Sie aufgehört haben, oder die Anmeldung neu beginnen.
jcPageExpiredContinue=Dort weitermachen, wo Sie aufgehört haben
jcPageExpiredRestart=Anmeldung neu beginnen
jcClientPortal=Portal
jcClientCatalogue=Open-Data-Katalog
jcClientParticipation=Beteiligungsplattform
jcClientForge=Code-Repository
jcClientMonitoring=Monitoring
loginAccountTitle=Anmelden
doLogIn=Anmelden
doForgotPassword=Passwort vergessen?
errorTitle=Etwas ist schiefgelaufen
invalidUserMessage=E-Mail oder Passwort stimmt nicht. Prüfen Sie beides und versuchen Sie es erneut.
missingUsernameMessage=Geben Sie Ihre E-Mail oder Ihren Benutzernamen ein.
missingPasswordMessage=Geben Sie Ihr Passwort ein.
loginTimeout=Die Anmeldung hat zu lange gedauert und beginnt neu. Geben Sie Ihre Daten noch einmal ein.
expiredCodeMessage=Die Anmeldung hat zu lange gedauert. Melden Sie sich erneut an.
expiredActionMessage=Dieser Link wurde schon benutzt oder ist abgelaufen. Melden Sie sich an, um fortzufahren.
expiredActionTokenNoSessionMessage=Dieser Link ist abgelaufen. Fordern Sie einen neuen an.
expiredActionTokenSessionExistsMessage=Dieser Link ist abgelaufen. Fordern Sie einen neuen an.
invalidCodeMessage=Die Anmeldung konnte nicht abgeschlossen werden. Öffnen Sie die Anwendung erneut und melden Sie sich dort an.
cookieNotFoundMessage=Ihr Browser hat das Anmelde-Cookie nicht behalten. Erlauben Sie Cookies für diese Seite und öffnen Sie die Anwendung erneut.
pageExpiredTitle=Diese Seite ist abgelaufen
emailForgotTitle=Passwort zurücksetzen
emailInstruction=Geben Sie Ihre E-Mail oder Ihren Benutzernamen ein. Wir senden Ihnen einen Link für ein neues Passwort.
emailInstructionUsername=Geben Sie Ihren Benutzernamen ein. Wir senden Ihnen einen Link für ein neues Passwort.
emailSentMessage=Wenn ein Konto passt, ist eine E-Mail mit Link unterwegs. Sehen Sie in Ihrem Posteingang nach.
backToLogin=Zurück zur Anmeldung
updatePasswordTitle=Neues Passwort wählen
emailVerifyTitle=E-Mail bestätigen
accountUpdatedMessage=Ihr Konto ist aktualisiert.
backToApplication=Zurück zur Anwendung
proceedWithAction=Weiter
logoutConfirmTitle=Abmelden
logoutConfirmHeader=Möchten Sie sich abmelden? Sie werden von allen Anwendungen dieser Organisation abgemeldet.
doLogout=Abmelden
invalidParameterMessage=Die Anwendung hat eine Anmeldung auf eine Weise angefragt, die diese Plattform nicht annimmt ({0}). Öffnen Sie die Anwendung erneut über ihren üblichen Link; wenn das wiederholt passiert, wenden Sie sich an Ihre Administration.
usernameOrEmailLabel=E-Mail oder Benutzername
usernameOrEmailHint=Zum Beispiel name@organisation.de
passwordLabel=Passwort
passwordHint=Mindestens 12 Zeichen mit Groß- und Kleinbuchstaben sowie einer Ziffer
otpLabel=Einmalcode
otpHint=6-stelliger Code aus Ihrer Authenticator-App (HMAC-SHA256)
otpEnrolTitle=Zwei-Faktor-Authentifizierung einrichten
otpEnrolInstruction=Fügen Sie diesen Schlüssel Ihrer Authenticator-App hinzu (zeitbasiert, {1}, {2} Ziffern, {3} s) und geben Sie den angezeigten Code ein: {0}
recoveryCodeLabel=Wiederherstellungscode
recoveryCodeHint=Geben Sie einen Ihrer 10 Einmal-Wiederherstellungscodes ein
orgChoiceTitle=Organisation auswählen
orgChoiceLabel=Organisation
orgChoiceHint=Wählen Sie die Organisation für diese Sitzung
consentTitle=Zugriff gewähren
consentInstruction=Diese Anwendung bittet um Zugriff auf Ihr Konto mit den angeforderten Berechtigungen.
termsTitle=Nutzungsbedingungen
termsInstruction=Akzeptieren Sie die Nutzungsbedingungen, um fortzufahren.
otpDeviceLabel=Gerätename
otpDeviceHint=Optional, zum Beispiel Diensthandy. Unter diesem Namen erscheint der Authenticator auf Ihrer Kontoseite.
doSubmit=Absenden
doCancel=Abbrechen
passwordNewLabel=Neues Passwort
passwordConfirmLabel=Passwort bestätigen
passwordConfirmHint=Geben Sie das neue Passwort noch einmal ein
notMatchPasswordMessage=Die Passwörter stimmen nicht überein. Geben Sie in beide Felder dasselbe Passwort ein.
logoutOtherSessions=Von anderen Geräten abmelden
logoutOtherSessionsHint=Jeder andere Browser und jede App, die mit Ihrem Konto angemeldet ist, muss sich neu anmelden.
reauthenticate=Melden Sie sich erneut an, um fortzufahren. So schützen wir Ihr Konto vor einer Sicherheitsänderung.
accountTitle=Konto
accountPersonalInfo=Persönliche Daten
accountPersonalInfoIntro=Verwalten Sie Ihre grundlegenden Daten.
accountSecurity=Kontosicherheit
accountSigningIn=Anmeldung
accountSigningInIntro=Legen Sie fest, wie Sie sich anmelden.
accountDeviceActivity=Geräteaktivität
accountDeviceActivityIntro=Sehen Sie, wo Sie angemeldet sind, und melden Sie sich von jedem Gerät ab.
accountUsername=Benutzername
accountEmail=E-Mail
accountEmailHint=Zum Beispiel name@organisation.de. Eine neue Adresse muss erneut bestätigt werden.
accountEmailUnverified=Diese Adresse ist noch nicht bestätigt.
accountFirstName=Vorname
accountLastName=Nachname
accountSave=Speichern
accountSaved=Ihr Konto wurde aktualisiert.
accountUpdate=Ändern
accountCreated=Erstellt
accountBasicAuth=Einfache Anmeldung
accountPasswordIntro=Melden Sie sich mit Ihrem Passwort an.
accountPasswordNone=Kein Passwort gesetzt.
accountTwoFactor=Zwei-Faktor-Authentifizierung
accountOtp=Authenticator-App
accountOtpIntro=Geben Sie einen Bestätigungscode aus Ihrer Authenticator-App ein.
accountOtpSetUp=Authenticator-App einrichten
accountOtpNone=Keine Authenticator-App eingerichtet.
accountOtpRequiredHint=Ihre Organisation verlangt eine Authenticator-App, daher kann die letzte nicht entfernt werden.
accountOtpRequired=Ihre Organisation verlangt eine Authenticator-App. Richten Sie eine weitere ein, bevor Sie diese entfernen.
accountPasskeys=Passkeys
accountPasskeysIntro=Melden Sie sich mit einem Passkey an.
accountRemove=Entfernen
accountRemoveConfirm={0} entfernen? Sie können sich damit nicht mehr anmelden.
accountRemoveConfirmButton=Ja, entfernen
accountRemoved=Die Anmeldemethode wurde entfernt.
accountCredentialNotRemovable=Diese Anmeldemethode kann nicht entfernt werden.
accountSignedInDevices=Angemeldete Geräte
accountCurrentSession=Aktuelle Sitzung
accountStarted=Beginn
accountLastAccess=Letzter Zugriff
accountExpires=Gültig bis
accountClients=Anwendungen
accountSignOut=Abmelden
accountSignOutAll=Alle Geräte abmelden
accountSignOutAllConfirm=Damit werden alle Geräte abgemeldet, auch dieses.
accountSignedOut=Das Gerät wurde abgemeldet.
accountSignOutLink=Abmelden
accountLoading=Wird geladen…
accountError=Etwas ist schiefgelaufen. Laden Sie die Seite neu und versuchen Sie es erneut.
accountActionSuccess=Ihre Änderung ist gespeichert.
accountActionCancelled=Es wurde nichts geändert.
accountMenu=Menü
accountSkip=Zum Inhalt springen
accountInvalidRequest=Die Änderung konnte nicht gelesen werden. Laden Sie die Seite neu und versuchen Sie es erneut.
accountFieldReadOnly=Dieses Feld kann hier nicht geändert werden.
accountFieldTooLong=Der Wert ist zu lang (höchstens 255 Zeichen).
accountFieldInvalid=Der Wert ist ungültig.
accountEmailRequired=Geben Sie Ihre E-Mail-Adresse ein.
accountEmailInvalid=Geben Sie eine gültige E-Mail-Adresse ein, zum Beispiel name@organisation.de.
accountEmailInUse=Ein anderes Konto verwendet diese E-Mail-Adresse bereits.
accountAttributeReadOnly=Dieses Attribut verwaltet Ihr Administrator; es kann hier nicht geändert werden.
"#;

const HU_PROPERTIES: &str = r#"
jcSignInTo=Bejelentkezés: {0} · {1}
jcSignInToApp=Bejelentkezés: {0}
jcBackTo=Vissza: {0}
jcOrganizationFallback=ez a platform
jcErrorNextStep=Zárja be ezt az oldalt, és nyissa meg újra az alkalmazást a hivatkozásáról. Ha ez ismétlődik, jelezze az időpontot a rendszergazdának.
jcPageExpiredExplained=Ez az oldal túl sokáig volt nyitva. Folytathatja ott, ahol abbahagyta, vagy újrakezdheti a bejelentkezést.
jcPageExpiredContinue=Folytatás ott, ahol abbahagyta
jcPageExpiredRestart=Bejelentkezés újrakezdése
jcClientPortal=Portál
jcClientCatalogue=Nyílt adatkatalógus
jcClientParticipation=Részvételi platform
jcClientForge=Kódtár
jcClientMonitoring=Monitoring
loginAccountTitle=Bejelentkezés
doLogIn=Bejelentkezés
doForgotPassword=Elfelejtette a jelszavát?
errorTitle=Hiba történt
invalidUserMessage=Az e-mail-cím vagy a jelszó nem megfelelő. Ellenőrizze mindkettőt, és próbálja újra.
missingUsernameMessage=Adja meg az e-mail-címét vagy felhasználónevét.
missingPasswordMessage=Adja meg a jelszavát.
loginTimeout=A bejelentkezés túl sokáig tartott, ezért elölről indul. Adja meg újra az adatait.
expiredCodeMessage=A bejelentkezés túl sokáig tartott. Jelentkezzen be újra.
expiredActionMessage=Ezt a hivatkozást már felhasználták, vagy lejárt. Jelentkezzen be a folytatáshoz.
expiredActionTokenNoSessionMessage=Ez a hivatkozás lejárt. Kérjen újat.
expiredActionTokenSessionExistsMessage=Ez a hivatkozás lejárt. Kérjen újat.
invalidCodeMessage=A bejelentkezést nem sikerült befejezni. Nyissa meg újra az alkalmazást, és jelentkezzen be onnan.
cookieNotFoundMessage=A böngésző nem őrizte meg a bejelentkezési sütit. Engedélyezze a sütiket ezen az oldalon, majd nyissa meg újra az alkalmazást.
pageExpiredTitle=Az oldal lejárt
emailForgotTitle=Jelszó visszaállítása
emailInstruction=Adja meg az e-mail-címét vagy felhasználónevét. Küldünk egy hivatkozást az új jelszó beállításához.
emailInstructionUsername=Adja meg a felhasználónevét. Küldünk egy hivatkozást az új jelszó beállításához.
emailSentMessage=Ha létezik ilyen fiók, az e-mail a hivatkozással már úton van. Ellenőrizze a postafiókját.
backToLogin=Vissza a bejelentkezéshez
updatePasswordTitle=Válasszon új jelszót
emailVerifyTitle=E-mail-cím megerősítése
accountUpdatedMessage=A fiókja frissült.
backToApplication=Vissza az alkalmazáshoz
proceedWithAction=Tovább
logoutConfirmTitle=Kijelentkezés
logoutConfirmHeader=Szeretne kijelentkezni? A szervezet minden alkalmazásából kijelentkezik.
doLogout=Kijelentkezés
invalidParameterMessage=Az alkalmazás olyan módon kérte a bejelentkezést, amelyet ez a platform nem fogad el ({0}). Nyissa meg újra az alkalmazást a szokásos hivatkozásáról; ha ez ismétlődik, jelezze a rendszergazdának.
usernameOrEmailLabel=E-mail vagy felhasználónév
usernameOrEmailHint=Például nev@szervezet.hu
passwordLabel=Jelszó
passwordHint=Legalább 12 karakter nagybetűvel, kisbetűvel és számmal
otpLabel=Egyszeri kód
otpHint=6 jegyű kód a hitelesítő alkalmazásból (HMAC-SHA256)
otpEnrolTitle=Kétlépcsős azonosítás beállítása
otpEnrolInstruction=Adja hozzá ezt a kulcsot a hitelesítő alkalmazásához (időalapú, {1}, {2} számjegy, {3} mp), majd írja be a megjelenő kódot: {0}
recoveryCodeLabel=Helyreállító kód
recoveryCodeHint=Adja meg a 10 egyszer használatos tartalék kód egyikét
orgChoiceTitle=Válasszon szervezetet
orgChoiceLabel=Szervezet
orgChoiceHint=Válassza ki a szervezetet ehhez a munkamenethez
consentTitle=Hozzáférés engedélyezése
consentInstruction=Ez az alkalmazás engedélyt kér a fiókja használatához a kért jogosultságokkal.
termsTitle=Felhasználási feltételek
termsInstruction=A folytatáshoz fogadja el a felhasználási feltételeket.
otpDeviceLabel=Eszköz neve
otpDeviceHint=Nem kötelező, például Munkahelyi telefon. Ezen a néven jelenik meg a hitelesítő a fiókoldalán.
doSubmit=Küldés
doCancel=Mégse
passwordNewLabel=Új jelszó
passwordConfirmLabel=Jelszó megerősítése
passwordConfirmHint=Írja be még egyszer az új jelszót
notMatchPasswordMessage=A két jelszó nem egyezik. Mindkét mezőbe ugyanazt a jelszót írja.
logoutOtherSessions=Kijelentkezés a többi eszközről
logoutOtherSessionsHint=Minden más böngészőnek és alkalmazásnak, amely be van jelentkezve a fiókjába, újra be kell jelentkeznie.
reauthenticate=A folytatáshoz jelentkezzen be újra. Így védjük a fiókját egy biztonsági módosítás előtt.
accountTitle=Fiók
accountPersonalInfo=Személyes adatok
accountPersonalInfoIntro=Kezelje alapvető adatait.
accountSecurity=Fiókbiztonság
accountSigningIn=Bejelentkezés
accountSigningInIntro=Állítsa be a bejelentkezés módjait.
accountDeviceActivity=Eszköztevékenység
accountDeviceActivityIntro=Nézze meg, hol van bejelentkezve, és jelentkezzen ki bármelyik eszközről.
accountUsername=Felhasználónév
accountEmail=E-mail
accountEmailHint=Például nev@szervezet.hu. Az új címet újra meg kell erősíteni.
accountEmailUnverified=Ez a cím még nincs megerősítve.
accountFirstName=Keresztnév
accountLastName=Vezetéknév
accountSave=Mentés
accountSaved=A fiókja frissült.
accountUpdate=Módosítás
accountCreated=Létrehozva
accountBasicAuth=Alapszintű hitelesítés
accountPasswordIntro=Jelentkezzen be a jelszavával.
accountPasswordNone=Nincs jelszó beállítva.
accountTwoFactor=Kétlépcsős azonosítás
accountOtp=Hitelesítő alkalmazás
accountOtpIntro=Adja meg a hitelesítő alkalmazás ellenőrző kódját.
accountOtpSetUp=Hitelesítő alkalmazás beállítása
accountOtpNone=Nincs hitelesítő alkalmazás beállítva.
accountOtpRequiredHint=A szervezete hitelesítő alkalmazást követel meg, ezért az utolsó nem távolítható el.
accountOtpRequired=A szervezete hitelesítő alkalmazást követel meg. Mielőtt ezt eltávolítja, állítson be egy másikat.
accountPasskeys=Jelszókulcsok
accountPasskeysIntro=Jelentkezzen be jelszókulccsal.
accountRemove=Eltávolítás
accountRemoveConfirm=Eltávolítja: {0}? Többé nem jelentkezhet be vele.
accountRemoveConfirmButton=Igen, eltávolítás
accountRemoved=A bejelentkezési mód eltávolítva.
accountCredentialNotRemovable=Ez a bejelentkezési mód nem távolítható el.
accountSignedInDevices=Bejelentkezett eszközök
accountCurrentSession=Jelenlegi munkamenet
accountStarted=Kezdete
accountLastAccess=Utolsó hozzáférés
accountExpires=Lejárat
accountClients=Alkalmazások
accountSignOut=Kijelentkezés
accountSignOutAll=Kijelentkezés minden eszközről
accountSignOutAllConfirm=Ez minden eszközt kijelentkeztet, ezt is beleértve.
accountSignedOut=Az eszköz kijelentkeztetve.
accountSignOutLink=Kijelentkezés
accountLoading=Betöltés…
accountError=Hiba történt. Töltse újra az oldalt, és próbálja újra.
accountActionSuccess=A módosítás mentve.
accountActionCancelled=Semmi sem változott.
accountMenu=Menü
accountSkip=Ugrás a tartalomra
accountInvalidRequest=A módosítás nem olvasható. Töltse újra az oldalt, és próbálja újra.
accountFieldReadOnly=Ez a mező itt nem módosítható.
accountFieldTooLong=Az érték túl hosszú (legfeljebb 255 karakter).
accountFieldInvalid=Az érték érvénytelen.
accountEmailRequired=Adja meg az e-mail-címét.
accountEmailInvalid=Adjon meg érvényes e-mail-címet, például nev@szervezet.hu.
accountEmailInUse=Ezt az e-mail-címet már egy másik fiók használja.
accountAttributeReadOnly=Ezt az attribútumot a rendszergazda kezeli, itt nem módosítható.
"#;

fn parse_properties(raw: &'static str) -> BTreeMap<&'static str, &'static str> {
    let mut map = BTreeMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            map.insert(k.trim(), v.trim());
        }
    }
    map
}

impl Bundle {
    #[must_use]
    pub fn for_locale(locale: &str) -> Self {
        match locale {
            "sk" => Self {
                locale: "sk",
                messages: parse_properties(SK_PROPERTIES),
            },
            "cs" => Self {
                locale: "cs",
                messages: parse_properties(CS_PROPERTIES),
            },
            "de" => Self {
                locale: "de",
                messages: parse_properties(DE_PROPERTIES),
            },
            "hu" => Self {
                locale: "hu",
                messages: parse_properties(HU_PROPERTIES),
            },
            _ => Self {
                locale: "en",
                messages: parse_properties(EN_PROPERTIES),
            },
        }
    }

    #[must_use]
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        self.messages.get(key).copied().unwrap_or(key)
    }

    #[must_use]
    pub fn format(&self, key: &str, args: &[&str]) -> String {
        let mut out = self.get(key).to_owned();
        for (i, arg) in args.iter().enumerate() {
            out = out.replace(&format!("{{{i}}}"), arg);
        }
        out
    }

    /// Translates `${jcClientPortal}` style client names from the theme (W7).
    #[must_use]
    pub fn translate_client_name(&self, raw_name: &str) -> String {
        if let Some(key) = raw_name
            .strip_prefix("${")
            .and_then(|s| s.strip_suffix('}'))
        {
            self.get(key).to_owned()
        } else {
            raw_name.to_owned()
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.messages.keys().copied()
    }
}

/// Locale negotiation order required by W7: `ui_locales` -> cookie -> `Accept-Language` -> realm default.
#[must_use]
pub fn negotiate_locale(
    ui_locales: Option<&str>,
    cookie_locale: Option<&str>,
    accept_language: Option<&str>,
    realm_default: &str,
) -> &'static str {
    let pick = |candidate: &str| -> Option<&'static str> {
        let primary = candidate
            .split(['-', '_', ';', ','])
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        SUPPORTED_LOCALES
            .iter()
            .copied()
            .find(|&loc| loc == primary)
    };

    if let Some(ui) = ui_locales {
        for part in ui.split_whitespace() {
            if let Some(found) = pick(part) {
                return found;
            }
        }
    }
    if let Some(ck) = cookie_locale
        && let Some(found) = pick(ck)
    {
        return found;
    }
    if let Some(al) = accept_language {
        for part in al.split(',') {
            if let Some(found) = pick(part) {
                return found;
            }
        }
    }
    pick(realm_default).unwrap_or("en")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_key_exists_in_all_five_shipped_languages_and_template_has_no_literal_strings()
    {
        let en = Bundle::for_locale("en");
        let en_keys: Vec<&str> = en.keys().collect();
        assert!(!en_keys.is_empty());

        for lang in SUPPORTED_LOCALES {
            let bundle = Bundle::for_locale(lang);
            for &key in &en_keys {
                let val = bundle.get(key);
                assert_ne!(
                    val, key,
                    "missing translation for key '{key}' in locale '{lang}'"
                );
                assert!(!val.is_empty());
            }
        }

        // Verify template has no literal user-facing words outside technical attributes
        let tpl = include_str!("../templates/login_page.html");
        for forbidden_literal in [
            "Sign in",
            "Forgot your password",
            "Something went wrong",
            "Enter your password",
        ] {
            assert!(
                !tpl.contains(forbidden_literal),
                "template contains literal user-facing string '{forbidden_literal}'"
            );
        }
    }

    #[test]
    fn negotiates_locale_in_w7_priority_order() {
        assert_eq!(
            negotiate_locale(Some("sk en"), Some("de"), Some("cs"), "en"),
            "sk"
        );
        assert_eq!(
            negotiate_locale(None, Some("hu"), Some("de-DE,de;q=0.9"), "en"),
            "hu"
        );
        assert_eq!(
            negotiate_locale(None, None, Some("cs-CZ,cs;q=0.9"), "en"),
            "cs"
        );
        assert_eq!(negotiate_locale(None, None, Some("fr-FR"), "de"), "de");
    }
}
