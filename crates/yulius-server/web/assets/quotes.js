// Quotes about building software, one shown at random above the new-task composer: lines from
// people programmers admire (and a good share of Richard Hamming). Only quotes with a
// well-documented author; widely misattributed lines are left out.

export const QUOTES = [
  { text: 'The best way to predict the future is to invent it.', author: 'Alan Kay' },
  { text: 'Simplicity is prerequisite for reliability.', author: 'Edsger W. Dijkstra' },
  { text: 'Make it work, make it right, make it fast.', author: 'Kent Beck' },
  { text: 'Talk is cheap. Show me the code.', author: 'Linus Torvalds' },
  { text: 'Make something people want.', author: 'Paul Graham' },
  { text: 'Programs must be written for people to read, and only incidentally for machines to execute.', author: 'Harold Abelson' },
  { text: 'Premature optimization is the root of all evil.', author: 'Donald Knuth' },
  { text: 'The most dangerous phrase in the language is “We’ve always done it this way.”', author: 'Grace Hopper' },
  { text: 'Simple things should be simple, complex things should be possible.', author: 'Alan Kay' },
  { text: 'The function of good software is to make the complex appear to be simple.', author: 'Grady Booch' },
  { text: 'Controlling complexity is the essence of computer programming.', author: 'Brian Kernighan' },
  { text: 'Release early. Release often. And listen to your customers.', author: 'Eric S. Raymond' },
  { text: 'The best code is no code at all.', author: 'Jeff Atwood' },
  { text: 'Make each program do one thing well.', author: 'Doug McIlroy' },
  { text: 'Testing shows the presence, not the absence of bugs.', author: 'Edsger W. Dijkstra' },
  { text: 'Optimism is an occupational hazard of programming: feedback is the treatment.', author: 'Kent Beck' },
  { text: 'If it hurts, do it more often.', author: 'Martin Fowler' },
  { text: 'Any fool can write code that a computer can understand. Good programmers write code that humans can understand.', author: 'Martin Fowler' },
  { text: 'Everyone knows that debugging is twice as hard as writing a program in the first place.', author: 'Brian Kernighan' },
  { text: 'When in doubt, use brute force.', author: 'Ken Thompson' },
  { text: 'One of my most productive days was throwing away 1,000 lines of code.', author: 'Ken Thompson' },
  { text: 'Data dominates. If you’ve chosen the right data structures and organized things well, the algorithms will almost always be self-evident.', author: 'Rob Pike' },
  { text: 'Simplicity is complicated.', author: 'Rob Pike' },
  { text: 'Adding manpower to a late software project makes it later.', author: 'Fred Brooks' },
  { text: 'Plan to throw one away; you will, anyhow.', author: 'Fred Brooks' },
  { text: 'There are two ways of constructing a software design: One way is to make it so simple that there are obviously no deficiencies and the other way is to make it so complicated that there are no obvious deficiencies.', author: 'C. A. R. Hoare' },
  { text: 'Simplicity does not precede complexity, but follows it.', author: 'Alan Perlis' },
  { text: 'A language that doesn’t affect the way you think about programming, is not worth knowing.', author: 'Alan Perlis' },
  { text: 'We will encourage you to develop the three great virtues of a programmer: laziness, impatience, and hubris.', author: 'Larry Wall' },
  { text: 'Bad programmers worry about the code. Good programmers worry about data structures and their relationships.', author: 'Linus Torvalds' },
  { text: 'Beware of bugs in the above code; I have only proved it correct, not tried it.', author: 'Donald Knuth' },
  { text: 'Code is read much more often than it is written.', author: 'Guido van Rossum' },
  { text: 'There are only two kinds of languages: the ones people complain about and the ones nobody uses.', author: 'Bjarne Stroustrup' },
  { text: 'Duplication is far cheaper than the wrong abstraction.', author: 'Sandi Metz' },
  { text: 'There are only two hard things in Computer Science: cache invalidation and naming things.', author: 'Phil Karlton' },
  { text: 'The hottest new programming language is English.', author: 'Andrej Karpathy' },
  { text: 'Some people, when confronted with a problem, think “I know, I’ll use regular expressions.” Now they have two problems.', author: 'Jamie Zawinski' },
  { text: 'A distributed system is one in which the failure of a computer you didn’t even know existed can render your own computer unusable.', author: 'Leslie Lamport' },
  { text: 'All problems in computer science can be solved by another level of indirection.', author: 'David Wheeler' },
  { text: 'Don’t comment bad code—rewrite it.', author: 'Brian W. Kernighan & P. J. Plauger' },
  // Richard W. Hamming, verbatim: Numerical Methods for Scientists and Engineers (1962), the talk
  // “You and Your Research” (Bell Labs, 1986) and The Art of Doing Science and Engineering (1997).
  { text: 'The purpose of computing is insight, not numbers.', author: 'Richard W. Hamming' },
  { text: 'In science, if you know what you are doing, you should not be doing it. In engineering, if you do not know what you are doing, you should not be doing it.', author: 'Richard W. Hamming' },
  { text: 'In a lifetime of many, many independent choices, small and large, a career with a vision will get you a distance proportional to n, while no vision will get you only the distance √n.', author: 'Richard W. Hamming' },
  { text: 'If you do not work on an important problem, it’s unlikely you’ll do important work.', author: 'Richard W. Hamming' },
  { text: 'It’s not the consequence that makes a problem important, it is that you have a reasonable attack.', author: 'Richard W. Hamming' },
  { text: 'Knowledge and productivity are like compound interest.', author: 'Richard W. Hamming' },
  { text: 'Great scientists tolerate ambiguity very well.', author: 'Richard W. Hamming' },
  { text: 'The particular thing you do is luck, but that you do something is not.', author: 'Richard W. Hamming' },
  { text: 'Given two people with exactly the same ability, the one person who manages day in and day out to get in one more hour of thinking will be tremendously more productive over a lifetime.', author: 'Richard W. Hamming' },
  { text: 'It is not sufficient to do a job, you have to sell it.', author: 'Richard W. Hamming' },
  { text: 'Most great scientists have tremendous drive.', author: 'Richard W. Hamming' },
  { text: 'One of the characteristics of successful scientists is having courage.', author: 'Richard W. Hamming' },
];

/** A random quote, avoiding the one shown last in this browser tab. */
let last = -1;
export function randomQuote() {
  let index = Math.floor(Math.random() * QUOTES.length);
  if (index === last) index = (index + 1) % QUOTES.length;
  last = index;
  return QUOTES[index];
}
