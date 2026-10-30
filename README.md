# Psephoi

An implementation of approval voting which is:

* **Verifiable**: You can prove that your vote was included in the tally
* **Secret**: No-one else can tell how you voted (not even the server operator)
* Relatively **low-tech**: It uses well-known crypto only

These properties are in tension with each other, but I 

## Introduction

### Verifiable voting

Perhaps the simplest possible voting system is open acclamation: everyone can
see who voted for whom.  This is a _verifiable_ voting system: since the list
of votes is public, you can check that your vote is recorded correctly.  You can
also re-count the votes yourself if you want to check that the tally is correct.

Open acclamation has a big drawback of course: people usually don't want their
vote to be publically known!  So here's the puzzle: we need to set things up so
that voters can verify the correctness of _their own_ vote, without being able
to see how _anyone else_ voted.  An interesting one!

### Existing methods

This is a problem which has been extensively studied, and there are even
methods which have been used for real.
See eg. Helios, Scantegrity, ThreeBallot, Pret a Voter, Scytl.

They're too complicated!  These systems have been implemented and audited and
in some cases even used for elections, and then people discover bugs in the
implementation which destroy the claimed properties.

### Online voting

Many existing methods rely on a physical polling station.  There's nothing wrong
with this per-se, but I'm interested to see how far we can get without one.
Needless to say, if people can vote on their phone, or via a website, it lowers
the barrier to entry for vote organisers.

Note that a physical polling station is probably the only real way to prevent
direct physical coersion.  If you can vote using an app on your phone, a
controlling father can always watch his children submit their vote.  That said,
see the section on "vote buying" for a potential mitigation.

### Approval voting

I'm specifically interested in approval voting.  Voters can vote for multiple
candidates.  They can vote for just their favourite, or their top 3, or
whatever they like; they can vote for _all_ of the candidates (although this is
equivalent to voting for none of them).  Every vote carries the same weight.  At
the end, the votes are tallied, and the candidate with the most votes wins.

You don't often see this style of voting amongst high-profile elections, but
it's popular amongst voting theorists.  It also has the property of being
extremely simple, and I want to know whether that simplicity can be exploited.

## The plan

### Overview

Ok, so here's my plan: it's a two-phase election, with a "registration"
phase and a "voting" phase.

In the registration phase, voters go to the _registrar_, identify themselves as
an eligible voter, and receive some voting tokens.  Each voter gets one token
per candidate, whether they intend to vote for them or not; so for example if
there are 100 voters and 3 candidates, the registrar will issue 300 tokens in
total.

A token represents a potential vote.  Tokens are clearly linked to a specific
candidate (and to a specific election), and can only be used to vote for that
candidate (and in that election).  Voters in possession of a token can choose
whether to destroy it or make it public.

In the voting phase, voters publish the tokens for the candidates they like,
and delete the tokens for the candidates they dislike.  All published tokens are
collected by the _ballot counter_.  Once the voting phase is over, the ballot
counter pushlishes the list of tokens they collected.

Voters can verify for themselves that their published tokens are indeed on
the list, and that their unpublished tokens are not.  In addition, anyone can
compute the final tally for each candidate, and check that all listed tokens are
valid and unique.

### Tokens and secrecy

What _are_ these tokens, exactly?  A token is a random number plus a
cryptographic signature.  When the election is announced, the registrar create
a number of keypairs---one per candidate.  A random number signed with one of
these keypairs can be used to vote for the corresponding candidate.  The public
keys are published, so when you receive your tokens you can check that you got
one valid token for each candidate.

When you actually make your vote, you submit your tokens to the ballot counter
anonymously.  You can use tor or something if you like.  But the registrar knows
who you are, because you had to authenticate to receive your tokens.  What's
to stop the registrar from remembering what tokens it gave to whom, and later
putting the pieces together and figuring out who you voted for?

There's a technique called "blind-signing".  In a blind-signing protocol, the
signer doesn't get to see what they're signing.  So: the voter generates a
random number and puts it in an envelope; the registrar signs the envelope,
without looking at the random number; then the voter unseals the envelope and
voila: they have a random number signed by the registrar, but the registrar
doesn't know what it is!

TODO: mention the browser-based thing.

### Efficient verification

The expectation is that most people will want to check their tokens after the
vote is finished, so we need to make this operation efficient.  Here's how:

After the voting phase finishes, the ballot counter arranges all the tokens
it collected in-order (sorted by the value of the random number).  It then
constructs a Merkle tree by hashing pairs of tokens, then hashing pairs of
hashes, etc. until it gets a final "root" hash.

Now, the ballot counter publically declares the results of the election.  This
comprises the tally and the root hash.  We can give the ballot counter its own
keypair to sign these results with.

Voters can now inquire about a certain token.  If the token is in the list,
the ballot counter returns a chain of hashes leading from the root to the
token. This chain serves as proof that the token is in the list.  This proof is
relatively quick to generate, and probably under a kilobyte in size.

If the token is _not_ in the list, the ballot counter returns the two tokens
either side of there that token _would_ be if it were, along with a chain of
hashes proving that those tokens are present and adjacent.  Again, the proof is
short and quickly produced.  In this case, the validity of the proof relies on
the list being sorted.  This is a property which energetic voters can verify by
downloading the entire list.

Note that it's perfectly safe to inquire about a token you didn't want to
release: you do expose the random number, but without the signature it's
useless.  Also, the election is already over.

The best part is that the list is public, so anyone can download it and start
generating proofs for others.  A proof generated by your neighbour is just as
good as one generated by the ballot counter itself.  You only need to know the
root hash.

## Threats

### Evil registrars

Let's boost our threat model: what if the registrar has been taken over by
one of the candidates?  They can pump out as many tokens as they like for that
candidate and submit them to the ballot counter, who would assume they come from
legitimate voters.

To guard against this, we can have multiple registrars.  Voters must get their
tokens signed by all registrars in order to use them for voting with.  Now, no
indivdual registrar can fabricate tokens---they would need to collaborate with
the other registrars.  If the registrars are antagonistic towards each other,
any attempt at collusion might be revealed to the public.

There's another opportunity for mischeif still open to the registrars though:
they can refuse to sign. Suppose a registrar has been captured by one of
the candidates.  When you ask to have your tokens signed, the registrar may
determine that you're likely to vote for the opponent, and refuse. This attack
can be foiled by another small modification: only require a token to be signed
by a _majority_ of registrars.

### Evil ballot counters

What happens if you get to the end of the election and your vote _hasn't_ been
included in the list? To be honest I'm not really sure what to do about this.

{%
We could make this kind of mischeif detectable sooner, by having the counter
issue a signed receipt when you submit your vote.  Then, if your tokens are
missing from the final list you could publish the receipt and prove the list is
bad.  But what if the counter simply refuses to take your tokens?  No receipt,
nothing.  It's the same result, but you know about it sooner.  And these
reciepts may open the door to easier vote buying, so I'm not sure it's a good
idea.
%}

We could pull the same trick again, and have multiple ballot counters.  The
final outcome of the election would be the union of the lists declared by all
counters.

### Vote buying

Secret ballots are useful for two reasons:

* They prevent retaliation against people who voted "wrong"
* They prevent people from selling their vote

The kind of secrecy we've been considering above prevents retaliation, I
believe.  Preventing vote-buying, however, is harder, because in this case the
voter themselves is a willing colluder.  If the voter is able to prove to a
third party that they voted in a certain way, then they can sell their vote.

The best mitigation I can think of is to allow people to nullify their vote
after voting. This means you could sell your vote (or be physically coerced),
and then later when you're out of sight you can spoil your ballot to prevent it
from counting.  You would do this by releasing all your tokens, in such a way
that they're counted in the tally, but not made visible in the final list.
