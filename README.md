# Psephoi

An implementation of approval voting which is:

* **Verifiable**: You can prove that your vote was included in the tally
* **Secret**: No-one else can tell how you voted (not even the server operator)
* Fairly **low-tech**: No fancy crypto required; you should be able to understand it

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

### Online voting

Many existing methods rely on a physical polling station.  There's nothing wrong
with this per-se, but I'm interested to see how far we can get without one.
Needless to say, if people can vote on their phone, or via a website, it lowers
the barrier to entry for vote organisers.

Note that a polling station is probably the only real way to prevent direct
physical coersion.  If you can vote using an app on your phone, a [controlling
parent][family voting] can always watch their children submit their vote
(for example).  That said, see the section on "vote buying" for a potential
mitigation.

[family voting]: https://en.wikipedia.org/wiki/Family_voting

### Approval voting

I'm specifically interested in approval voting.  Voters can vote for multiple
candidates.  They can vote for just their favourite, or their top 3, or
whatever they like; they can vote for _all_ of the candidates (although this is
equivalent to voting for none of them).  Every vote carries the same weight.  At
the end, the votes are tallied, and the candidate with the most votes wins.

You don't often see this style of voting amongst high-profile elections, but
it's popular amongst voting theorists.  It also has the property of being
extremely simple, and I want to know whether that simplicity can be exploited.

### Existing methods

The question of verifiable and secret elections has been extensively studied.
There are many proposed methods, and some of them have even been used for real
elections.  My problem with these methods is that they're too complicated!  They
use niche crypto with no standard implementations, so the code is all bespoke
and hard to audit.
This is not just my opinion: there's [a history in bugs][bugs] in these systems
which undermine the claimed properties.

[bugs]: https://www.microsoft.com/en-us/research/video/how-not-to-prove-your-election-outcome/

<!-- For some examples, take a look at Helios, Scantegrity, ThreeBallot, Pret a Voter, and Scytl. -->

So my goal is a scheme that any programmer can understand and implement, at
least in a basic unhardened form.  A production-ready implementation still
requires care of course (eg. to avoid side-channels), but it should still be
easy enough to see whether it implements the core scheme correctly.

## The plan

### Overview

You start by registering to vote.  To do so, you must identify yourself to the
_registrar_, which checks that you're eligible to vote.  It will then issue
you a set of voting tokens.  You get one token per candidate, no matter who you
intend to vote for.  So, for example, if there are 100 voters and 3 candidates,
the registrar will issue 300 tokens in total.

A token represents a potential vote.  Tokens are clearly linked to a specific
candidate (and to a specific election), and can only be used to vote for that
candidate (and in that election).  Voters in possession of a token can choose
whether to destroy it or make it public.

The _ballot counter_ records any tokens which are sent to it.  For any
candidates you like, send that candidate's token to the ballot counter.  For
any candidates you _dislike_, keep the token secret until the election is over
(you can just delete them if you want).  Once the election is over, the ballot
counter publishes the list of tokens they collected, and that's the result of
the election.

Voters can verify for themselves that their published tokens are indeed on
the list, and that their unpublished tokens are not.  In addition, anyone can
compute the final tally for each candidate, and check that all listed tokens are
valid and unique.

### Tokens and secrecy

What _are_ these tokens, exactly?  A token is a random number plus a
cryptographic signature.

When the election is announced, the registrar generates one keypair per
candidate, and announces the public keys.  Any number signed with a candidate's
key can be used to vote for that candidate.  When you collect your tokens you
can check that you got one valid token for each candidate.

When you make your vote, you anonymously submit some tokens to the ballot
counter.  You can use tor or OHTTP to avoid exposing your IP.  But what about
the registrar?  It knows who you are; what's to stop it from remembering the
tokens it gave you and observing your vote?

There's a technique called "blind-signing".  In a blind-signing protocol, the
signer doesn't get to see what they're signing.  The classic analogy is an
envelope lined with carbon paper. The voter generates a random number and puts
it in an envelope; the registrar signs the envelope, without opening it; then
the voter unseals the envelope and voila: they have a random number signed by
the registrar, but the registrar doesn't know what it is!

The technique was introduced by David Chaum in 1983, but was relatively little
used until 2021 when people (myself included) suddenly started taking notice of
it.  The crypto was standardized by the IETF in 2023 ([RFC 9474]), and work to
incorporate it into the browser (Privacy Pass) is [ongoing][privacypass].  There
are multiple implementations available, and I think it's fair to say that it
numbers amonst the well-known techniques in crypto at this point.

[RFC 9474]: https://www.rfc-editor.org/info/rfc9474/
[privacypass]: https://datatracker.ietf.org/wg/privacypass/about/

### Election phases

The registrar knows who you are.  If it sees you collecting your tokens, and
then milliseconds later someone publishes their vote, the registrar can put two
and two together and make a confident guess that it was you voting.

To avoid this we break the election into two phases: the "registration" phase,
where voters collect their tokens, and the "voting" phase, where they publish
the tokens they want to publish.  If these phases don't overlap, then it's
impossible to correlate registrations with votes.

(In this implementation, disjoint phases are optional, but should only be
disabled when secrecy is not required.)

## Implementation details

### Efficient verification, take 1

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

### Efficient verification, take 2

There's a very relevant piece of prior art: Certificate Transparency logs.
Clients can query a certain certificate, and if the certificate is in the
log then the server will return a proof of inclusion. Like me, they chose to
structure their logs as binary Merkle trees, in order to minimise the length of
the inclusion proof. (The key difference is that the leaves of their tree are
in insertion order, which makes it easy to verify that a successor tree contains
its predecessor, but means they can't do non-inclusion proofs.)

In 2025, a consensus emerged amongst CT log operators that a binary tree was
a bit of a mistake, and that they really would have preferred a 256-ary tree.
With a fanout of 256, the nodes become big enough and _few_ enough that you can
store them as separate S3 objects.  Rather than having the server generate an
inclusion proof on-demand (compute-intensive), the clients can generate their
own by downloading the required objects.  Clients end up downloading many times
more data than before, but in the grand scheme of things it's still not a lot of
data. And the server no longer has anything to do: it's just static files, and
the CDN handles scaling. It turns out that dumb distribution, not proof size,
was the thing to optimize for.

By the way, the CT people were able to retroactively turn their binary trees
into 256-ary trees by redefining the hash function: instead of taking the
SHA-256 of the node, you compute its hash by simulating the old binary tree
(hash pairs of leaves, hash pairs of hashes, etc.).  This means you get the same
root hash whether you use the new method or the old method - very cool.

### Efficient verification, take 3

Which hash to use?  These days the standard option for new projects is blake3.
Blake3 is a tree hash, which works by breaking the data into blocks, and building
a Merkle tree with those blocks as leaves, so that the hash of the data is the
root hash of the Merkle tree, and... hang on a minute!

Blake3 does this for performance reasons (you can hash the blocks in parallel),
but it hasn't escaped the notice of the blake people that Merkle trees allow
you to do inclusion proofs. They call it "verified streaming", but it's the same
idea. And they've even standardised a format for storing the tree's inner nodes:
"the bao outboard format", or `.obao` files.

So, here's the game plan:  after the election is over, the counter publishes
a file for each candidate.  The file contains the random numbers from all the
tokens the counter received for that candidate, sorted and unique.  The counter
announces the blake3 hashes of these files, and signs the announcement. They
also publish files containing the token signatures.  A full audit would require
downloading both. But if you're just a voter trying to verify your own vote,
all you need is the former.  You do binary search (or interpolation search) via
HTTP range requests do get to the chunk containing your number (or that _would_
contain your number, if you'd submitted it).  Then you use the obao file to
verify that the chunk is consistent with the announced hash.

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

When you submit your vote to the ballot counter it issues you a signed receipt.
If you get to the end of the election and your vote _hasn't_ been included in
the list, you can prove that it wasn't an innocent mistake by revealing your
receipt.

But what if the counter simply refuses to take your tokens?  No receipt,
nothing.  Then you're stuck.  You might say that the receipts bought nothing;
but they do make things slightly more awkward for an evil counter: they have to
commit to being evil up-front, before they've seen the results of the election.
Without the receipts the counter could wait and see.

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
