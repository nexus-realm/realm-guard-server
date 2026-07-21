# Notes de sécurité — realm-guard-server

Le serveur ne fait que **stocker et relayer des données chiffrées de bout en bout**.
Il n'apprend jamais le mot de passe maître (authentification **OPAQUE**, aPAKE
zero-knowledge) ni le contenu du coffre. Ce document consigne le **modèle de
menace** et les **compromis acceptés** — à tenir à jour avec le code.

## Compromis accepté : énumération de comptes à l'inscription

`POST /auth/register/start` renvoie **409 Conflict** si le `username` est déjà
pris, et **200** sinon. Cela permet donc de **déterminer si un compte existe**
(énumération), en interrogeant cet endpoint username par username.

**Pourquoi c'est accepté.** L'inscription doit pouvoir dire à l'utilisateur « ce
nom est déjà pris » — c'est une exigence d'UX fondamentale. Masquer l'information
(ex. réponse générique + e-mail de confirmation différée) reporterait l'erreur,
compliquerait le flux, et n'empêcherait pas un attaquant déterminé de recouper
l'existence d'un compte par d'autres canaux (timing, comportement au `finish`).

**Ce qui borne le risque.**

- La **vitesse** d'énumération est plafonnée par le **rate-limit par-IP** au
  reverse proxy (Caddy, cf. `Caddyfile`) : l'énumération de masse est ralentie.
- Le **login** (`/auth/login/{start,finish}`), lui, **reste anti-énumération** :
  pour un utilisateur inconnu, le serveur fabrique une réponse indistinguable
  d'un vrai compte (le client échoue au `finish`), et le rate-limit par-compte
  est appliqué **sur le username tel qu'envoyé** (existant ou non) → verrouillage
  identique dans les deux cas.

**Risque résiduel.** Un attaquant peut apprendre qu'un identifiant donné possède
un compte Realm Guard. C'est une fuite de **présence de compte**, pas d'un secret :
aucun mot de passe ni aucune donnée de coffre n'est exposé. Jugé acceptable au
regard du coût/bénéfice.

## Défenses en place (rappel)

- **OPAQUE** : le serveur ne voit jamais le mot de passe ; le `password_file`
  stocké est inutilisable sans compléter le protocole (KSF Argon2id 64 MiB/t3).
- **Secret serveur OPAQUE isolé** : le `ServerSetup` (graine OPRF) est fourni
  **hors base** (`RG_OPAQUE_SETUP_FILE` / `RG_OPAQUE_SETUP`), jamais co-localisé
  avec les `password_file` → un dump de la base ne suffit pas à une attaque
  dictionnaire hors-ligne.
- **Rate-limit anti-brute-force** : **par compte** dans l'app (échecs de
  `login/finish`, verrouillage Redis) + **par IP** au proxy (volumétrique).
- **Sessions** : tokens opaques en Redis, **révocables**, TTL 7 j.

## Exigences de déploiement

- **TLS obligatoire** en production (le token de session Bearer transite sinon en
  clair). Terminaison TLS au reverse proxy.
- Fournir `RG_OPAQUE_SETUP_FILE` (secret monté) ; **ne jamais régénérer** le
  `ServerSetup` (invaliderait tous les comptes).

## Signalement

Signaler une vulnérabilité en privé au mainteneur (ticket privé / contact direct)
— **pas** via une issue publique.
