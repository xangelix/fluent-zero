-brand = Acme
    .gender = neuter

plain = Hello
quoted = Say "hi" to C:\Users
multiline =
    First line
    second line
literal-brace = Open {"{"} here
number = Take { 5 } now
about = About { -brand }
pronoun = { -brand.gender ->
    [neuter] it
   *[other] they
}
nested = { about }!
greeting = Hello { $name }
count = { $n ->
    [one] one file
   *[other] { $n } files
}
login = Log in
    .tooltip = Sign in to { -brand }
    .greeting = Welcome back, { $name }
    .en-only = Only in English
search =
    .placeholder = Search { -brand }
